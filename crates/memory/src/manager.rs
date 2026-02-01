/// Memory manager: orchestrates file sync, chunking, embedding, and search.
use std::path::Path;

use {
    sha2::{Digest, Sha256},
    tracing::{debug, info, warn},
    walkdir::WalkDir,
};

use crate::{
    chunker::chunk_markdown,
    config::MemoryConfig,
    embeddings::EmbeddingProvider,
    schema::{ChunkRow, FileRow},
    search::{self, SearchResult},
    store::MemoryStore,
};

pub struct MemoryManager {
    config: MemoryConfig,
    store: Box<dyn MemoryStore>,
    embedder: Option<Box<dyn EmbeddingProvider>>,
}

/// Status info about the memory system.
#[derive(Debug, Clone)]
pub struct MemoryStatus {
    pub total_files: usize,
    pub total_chunks: usize,
    pub embedding_model: String,
}

impl MemoryManager {
    /// Create a memory manager with an embedding provider for hybrid (vector + keyword) search.
    pub fn new(
        config: MemoryConfig,
        store: Box<dyn MemoryStore>,
        embedder: Box<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            config,
            store,
            embedder: Some(embedder),
        }
    }

    /// Create a memory manager without embeddings. Keyword (FTS) search only.
    pub fn keyword_only(config: MemoryConfig, store: Box<dyn MemoryStore>) -> Self {
        Self {
            config,
            store,
            embedder: None,
        }
    }

    /// Whether this manager has an embedding provider for vector search.
    pub fn has_embeddings(&self) -> bool {
        self.embedder.is_some()
    }

    /// Synchronize: walk configured directories, detect changed files, re-chunk and re-embed.
    pub async fn sync(&self) -> anyhow::Result<SyncReport> {
        let mut report = SyncReport::default();

        let mut discovered_paths = Vec::new();

        for dir in &self.config.memory_dirs {
            if !dir.exists() {
                debug!(?dir, "memory directory does not exist, skipping");
                continue;
            }

            for entry in WalkDir::new(dir).follow_links(true).into_iter().flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext != "md" && ext != "markdown" {
                    continue;
                }

                let path_str = path.to_string_lossy().to_string();
                discovered_paths.push(path_str.clone());

                match self.sync_file(path, &path_str).await {
                    Ok(changed) => {
                        if changed {
                            report.files_updated += 1;
                        } else {
                            report.files_unchanged += 1;
                        }
                    },
                    Err(e) => {
                        warn!(path = %path_str, error = %e, "failed to sync file");
                        report.errors += 1;
                    },
                }
            }
        }

        // Remove files no longer on disk
        let existing_files = self.store.list_files().await?;
        for file in existing_files {
            if !discovered_paths.contains(&file.path) {
                info!(path = %file.path, "removing deleted file from memory");
                self.store.delete_chunks_for_file(&file.path).await?;
                self.store.delete_file(&file.path).await?;
                report.files_removed += 1;
            }
        }

        Ok(report)
    }

    /// Sync a single file. Returns true if it was updated.
    async fn sync_file(&self, path: &Path, path_str: &str) -> anyhow::Result<bool> {
        let content = tokio::fs::read_to_string(path).await?;
        let hash = sha256_hex(&content);
        let metadata = tokio::fs::metadata(path).await?;
        let mtime = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let size = metadata.len() as i64;

        // Check if file is unchanged
        if let Some(existing) = self.store.get_file(path_str).await?
            && existing.hash == hash
        {
            return Ok(false);
        }

        // Determine source from path
        let source = if path_str.contains("MEMORY") {
            "longterm"
        } else {
            "daily"
        };

        // Update file record
        let file_row = FileRow {
            path: path_str.to_string(),
            source: source.to_string(),
            hash: hash.clone(),
            mtime,
            size,
        };
        self.store.upsert_file(&file_row).await?;

        // Chunk the content
        let raw_chunks =
            chunk_markdown(&content, self.config.chunk_size, self.config.chunk_overlap);

        // Delete old chunks
        self.store.delete_chunks_for_file(path_str).await?;

        // Generate embeddings (if provider available) and create chunk rows.
        let texts: Vec<String> = raw_chunks.iter().map(|c| c.text.clone()).collect();
        let (embeddings, model_name) = if let Some(ref embedder) = self.embedder {
            let embs = embedder.embed_batch(&texts).await?;
            (Some(embs), embedder.model_name().to_string())
        } else {
            (None, String::new())
        };

        let chunk_rows: Vec<ChunkRow> = raw_chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                let chunk_hash = sha256_hex(&chunk.text);
                let emb_blob = embeddings.as_ref().map(|embs| {
                    embs[i]
                        .iter()
                        .flat_map(|f| f.to_le_bytes())
                        .collect::<Vec<u8>>()
                });
                ChunkRow {
                    id: format!("{}:{}", path_str, i),
                    path: path_str.to_string(),
                    source: source.to_string(),
                    start_line: chunk.start_line as i64,
                    end_line: chunk.end_line as i64,
                    hash: chunk_hash,
                    model: model_name.clone(),
                    text: chunk.text.clone(),
                    embedding: emb_blob,
                    updated_at: chrono_now(),
                }
            })
            .collect();

        self.store.upsert_chunks(&chunk_rows).await?;
        info!(path = %path_str, chunks = chunk_rows.len(), "synced file");

        Ok(true)
    }

    /// Search memory. Uses hybrid (vector + keyword) when embeddings are available,
    /// falls back to keyword-only search otherwise.
    pub async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        if let Some(ref embedder) = self.embedder {
            search::hybrid_search(
                self.store.as_ref(),
                embedder.as_ref(),
                query,
                limit,
                self.config.vector_weight,
                self.config.keyword_weight,
            )
            .await
        } else {
            search::keyword_only_search(self.store.as_ref(), query, limit).await
        }
    }

    /// Get a specific chunk by ID.
    pub async fn get_chunk(&self, id: &str) -> anyhow::Result<Option<ChunkRow>> {
        self.store.get_chunk_by_id(id).await
    }

    /// Get status information about the memory system.
    pub async fn status(&self) -> anyhow::Result<MemoryStatus> {
        let files = self.store.list_files().await?;
        let mut total_chunks = 0usize;
        for file in &files {
            let chunks = self.store.get_chunks_for_file(&file.path).await?;
            total_chunks += chunks.len();
        }
        Ok(MemoryStatus {
            total_files: files.len(),
            total_chunks,
            embedding_model: self
                .embedder
                .as_ref()
                .map(|e| e.model_name().to_string())
                .unwrap_or_else(|| "none (keyword-only)".into()),
        })
    }
}

/// Sync report.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub files_updated: usize,
    pub files_unchanged: usize,
    pub files_removed: usize,
    pub errors: usize,
}

fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn chrono_now() -> String {
    // Simple ISO 8601 timestamp without chrono dependency
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{schema::run_migrations, store_sqlite::SqliteMemoryStore},
        async_trait::async_trait,
        std::io::Write,
        tempfile::TempDir,
    };

    /// Mock embedding provider that produces deterministic vectors from content.
    ///
    /// Uses a simple bag-of-keywords approach: each of 8 dimensions corresponds to a
    /// keyword. If the text contains that keyword the dimension is 1.0, otherwise 0.0.
    /// This lets vector search distinguish topics in tests.
    struct MockEmbedder;

    const KEYWORDS: [&str; 8] = [
        "rust", "python", "database", "memory", "search", "network", "cooking", "music",
    ];

    fn keyword_embedding(text: &str) -> Vec<f32> {
        let lower = text.to_lowercase();
        KEYWORDS
            .iter()
            .map(|kw| {
                if lower.contains(kw) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbedder {
        async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(keyword_embedding(text))
        }

        fn model_name(&self) -> &str {
            "mock-model"
        }

        fn dimensions(&self) -> usize {
            8
        }
    }

    async fn setup() -> (MemoryManager, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();

        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();

        let config = MemoryConfig {
            db_path: ":memory:".into(),
            memory_dirs: vec![mem_dir],
            chunk_size: 50,
            chunk_overlap: 10,
            vector_weight: 0.7,
            keyword_weight: 0.3,
        };

        let store = Box::new(SqliteMemoryStore::new(pool));
        let embedder = Box::new(MockEmbedder);

        (MemoryManager::new(config, store, embedder), tmp)
    }

    #[tokio::test]
    async fn test_sync_and_search() {
        let (manager, tmp) = setup().await;
        let mem_dir = tmp.path().join("memory");

        // Create a test file
        let mut f = std::fs::File::create(mem_dir.join("2024-01-01.md")).unwrap();
        writeln!(f, "# Daily Log").unwrap();
        writeln!(f, "Today I worked on the Rust memory system.").unwrap();
        writeln!(f, "It uses SQLite for storage and hybrid search.").unwrap();

        // Sync
        let report = manager.sync().await.unwrap();
        assert_eq!(report.files_updated, 1);
        assert_eq!(report.files_unchanged, 0);

        // Sync again - should be unchanged
        let report2 = manager.sync().await.unwrap();
        assert_eq!(report2.files_updated, 0);
        assert_eq!(report2.files_unchanged, 1);

        // Status
        let status = manager.status().await.unwrap();
        assert_eq!(status.total_files, 1);
        assert!(status.total_chunks > 0);
        assert_eq!(status.embedding_model, "mock-model");
    }

    #[tokio::test]
    async fn test_sync_detects_changes() {
        let (manager, tmp) = setup().await;
        let mem_dir = tmp.path().join("memory");
        let file_path = mem_dir.join("test.md");

        std::fs::write(&file_path, "version 1").unwrap();
        let r1 = manager.sync().await.unwrap();
        assert_eq!(r1.files_updated, 1);

        std::fs::write(&file_path, "version 2 with different content").unwrap();
        let r2 = manager.sync().await.unwrap();
        assert_eq!(r2.files_updated, 1);
    }

    #[tokio::test]
    async fn test_sync_removes_deleted_files() {
        let (manager, tmp) = setup().await;
        let mem_dir = tmp.path().join("memory");
        let file_path = mem_dir.join("temp.md");

        std::fs::write(&file_path, "temporary content").unwrap();
        manager.sync().await.unwrap();

        std::fs::remove_file(&file_path).unwrap();
        let report = manager.sync().await.unwrap();
        assert_eq!(report.files_removed, 1);
    }

    /// End-to-end: sync markdown files, then search and verify the returned text
    /// matches what was written.
    #[tokio::test]
    async fn test_search_returns_synced_content() {
        let (manager, tmp) = setup().await;
        let mem_dir = tmp.path().join("memory");

        std::fs::write(
            mem_dir.join("2024-01-15.md"),
            "# Rust and memory\nToday I built a Rust memory system with search capabilities.",
        )
        .unwrap();

        manager.sync().await.unwrap();

        // Search for "rust memory" — should return the chunk we just synced
        let results = manager.search("rust memory", 5).await.unwrap();
        assert!(!results.is_empty(), "search should return results");
        let texts: Vec<&str> = results.iter().map(|r| r.text.as_str()).collect();
        let combined = texts.join(" ");
        assert!(
            combined.contains("Rust memory system"),
            "search results should contain the synced text, got: {combined}"
        );
    }

    /// Keyword (FTS) search works through the manager after sync.
    #[tokio::test]
    async fn test_keyword_search_through_manager() {
        let (manager, tmp) = setup().await;
        let mem_dir = tmp.path().join("memory");

        std::fs::write(
            mem_dir.join("log.md"),
            "Rust programming is great for building fast systems.",
        )
        .unwrap();

        manager.sync().await.unwrap();

        // Keyword search bypasses embeddings—FTS5 MATCH query
        let results = manager.search("programming", 5).await.unwrap();
        assert!(
            !results.is_empty(),
            "keyword search should find 'programming'"
        );
        assert!(
            results[0].text.contains("programming"),
            "top result should contain the search term"
        );
    }

    /// Multiple files with distinct topics: searching for one topic should rank that
    /// file's chunks higher than unrelated files.
    #[tokio::test]
    async fn test_multi_file_topic_separation() {
        let (manager, tmp) = setup().await;
        let mem_dir = tmp.path().join("memory");

        std::fs::write(
            mem_dir.join("rust.md"),
            "Rust is a systems programming language focused on safety and performance.",
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("cooking.md"),
            "Today I tried a new cooking recipe for pasta with garlic and olive oil.",
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("music.md"),
            "Listened to music all afternoon. Jazz and classical music are relaxing.",
        )
        .unwrap();

        manager.sync().await.unwrap();

        let status = manager.status().await.unwrap();
        assert_eq!(status.total_files, 3);

        // Search for "rust" — the rust.md chunk should come first
        let results = manager.search("rust", 5).await.unwrap();
        assert!(!results.is_empty());
        assert!(
            results[0].path.contains("rust.md"),
            "top result for 'rust' should come from rust.md, got: {}",
            results[0].path
        );

        // Search for "cooking" — the cooking.md chunk should come first
        let results = manager.search("cooking", 5).await.unwrap();
        assert!(!results.is_empty());
        assert!(
            results[0].path.contains("cooking.md"),
            "top result for 'cooking' should come from cooking.md, got: {}",
            results[0].path
        );

        // Search for "music" — the music.md chunk should come first
        let results = manager.search("music", 5).await.unwrap();
        assert!(!results.is_empty());
        assert!(
            results[0].path.contains("music.md"),
            "top result for 'music' should come from music.md, got: {}",
            results[0].path
        );
    }

    /// Sync many files and verify search still completes (basic scale sanity check).
    #[tokio::test]
    async fn test_scale_many_files() {
        let (manager, tmp) = setup().await;
        let mem_dir = tmp.path().join("memory");

        // Create 50 files, each with several lines
        for i in 0..50 {
            let topic = &KEYWORDS[i % KEYWORDS.len()];
            let mut content = format!("# File {i} about {topic}\n\n");
            for j in 0..20 {
                content.push_str(&format!(
                    "Line {j}: This paragraph discusses {topic} in detail with enough words to fill a line.\n"
                ));
            }
            std::fs::write(mem_dir.join(format!("file_{i:03}.md")), &content).unwrap();
        }

        let report = manager.sync().await.unwrap();
        assert_eq!(report.files_updated, 50);

        let status = manager.status().await.unwrap();
        assert_eq!(status.total_files, 50);
        assert!(
            status.total_chunks >= 50,
            "should have at least one chunk per file, got {}",
            status.total_chunks
        );

        // Search should still return results
        let results = manager.search("database", 10).await.unwrap();
        assert!(
            !results.is_empty(),
            "search across 50 files should return results"
        );

        // All top results should be about database
        for r in &results {
            assert!(
                r.text.to_lowercase().contains("database"),
                "result should be about database, got: {}",
                r.text.chars().take(80).collect::<String>()
            );
        }
    }

    /// Keyword-only mode: sync and search without any embedding provider.
    #[tokio::test]
    async fn test_keyword_only_mode() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();

        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();

        let config = MemoryConfig {
            db_path: ":memory:".into(),
            memory_dirs: vec![mem_dir.clone()],
            chunk_size: 50,
            chunk_overlap: 10,
            ..Default::default()
        };

        let store = Box::new(SqliteMemoryStore::new(pool));
        let manager = MemoryManager::keyword_only(config, store);

        assert!(!manager.has_embeddings());

        // Write a test file and sync (should work without embeddings).
        std::fs::write(
            mem_dir.join("note.md"),
            "Rust programming is great for building fast systems.",
        )
        .unwrap();

        let report = manager.sync().await.unwrap();
        assert_eq!(report.files_updated, 1);

        let status = manager.status().await.unwrap();
        assert_eq!(status.total_files, 1);
        assert!(status.total_chunks > 0);
        assert_eq!(status.embedding_model, "none (keyword-only)");

        // Keyword search should still work.
        let results = manager.search("programming", 5).await.unwrap();
        assert!(
            !results.is_empty(),
            "keyword-only search should find results"
        );
        assert!(results[0].text.contains("programming"));
    }

    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex("hello");
        assert_eq!(hash.len(), 64);
        // Known SHA-256 of "hello"
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
