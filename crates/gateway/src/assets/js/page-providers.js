// ── Providers page (Preact + HTM + Signals) ─────────────────

import { signal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect } from "preact/hooks";
import { sendRpc } from "./helpers.js";
import { fetchModels } from "./models.js";
import { updateNavCount } from "./nav-counts.js";
import { openProviderModal } from "./providers.js";
import { registerPage } from "./router.js";
import { connected } from "./signals.js";
import * as S from "./state.js";
import { ConfirmDialog, requestConfirm } from "./ui.js";

var providers = signal([]);
var loading = signal(false);

function fetchProviders() {
	loading.value = true;
	sendRpc("providers.available", {}).then((res) => {
		loading.value = false;
		if (!res?.ok) return;
		providers.value = (res.payload || [])
			.filter((p) => p.configured)
			.sort((a, b) => a.displayName.localeCompare(b.displayName));
		updateNavCount("providers", providers.value.length);
	});
}

function ProviderCard(props) {
	var p = props.provider;

	function onRemove() {
		requestConfirm(`Remove credentials for ${p.displayName}?`).then((yes) => {
			if (!yes) return;
			sendRpc("providers.remove_key", { provider: p.name }).then((res) => {
				if (res?.ok) {
					fetchModels();
					fetchProviders();
				}
			});
		});
	}

	return html`<div class="provider-card">
    <div style="display:flex;align-items:center;gap:8px;">
      <span class="text-sm text-[var(--text-strong)]">${p.displayName}</span>
      <span class="provider-item-badge ${p.authType}">
        ${p.authType === "oauth" ? "OAuth" : "API Key"}
      </span>
    </div>
    <button class="session-action-btn session-delete" title="Remove ${p.displayName}" onClick=${onRemove}>Remove</button>
  </div>`;
}

function ProvidersPage() {
	useEffect(() => {
		fetchProviders();
	}, []);

	S.setRefreshProvidersPage(fetchProviders);

	return html`
    <div class="flex-1 flex flex-col min-w-0 p-4 gap-4 overflow-y-auto">
      <div class="flex items-center gap-3">
        <h2 class="text-lg font-medium text-[var(--text-strong)]">Providers</h2>
        <button class="bg-[var(--accent-dim)] text-white border-none px-3 py-1.5 rounded text-xs cursor-pointer hover:bg-[var(--accent)] transition-colors"
          onClick=${() => {
						if (connected.value) openProviderModal();
					}}>+ Add Provider</button>
      </div>
      ${
				loading.value && providers.value.length === 0
					? html`<div class="text-sm text-[var(--muted)]">Loading\u2026</div>`
					: providers.value.length === 0
						? html`<div class="text-sm text-[var(--muted)]">No providers connected yet.</div>`
						: providers.value.map((p) => html`<${ProviderCard} key=${p.name} provider=${p} />`)
			}
    </div>
    <${ConfirmDialog} />
  `;
}

registerPage(
	"/providers",
	function initProviders(container) {
		container.style.cssText = "flex-direction:column;padding:0;overflow:hidden;";
		render(html`<${ProvidersPage} />`, container);
	},
	function teardownProviders() {
		S.setRefreshProvidersPage(null);
		var container = S.$("pageContent");
		if (container) render(null, container);
	},
);
