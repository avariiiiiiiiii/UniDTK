import { invoke } from '@unidtk/bridge';
import { showDebugMessage, logFrontend, displayText, escapeHtml } from '../../unidtk-core.js';
import { surfaceManager } from '../../surface-manager.js';

let vpnServersCache = null;
let vpnBusy = false;
let vpnPingResults = null; // Map<tag, { latencyMs, ok }> from the last "PING ALL"
let vpnPinging = false;
let isUpdatingAutoConnect = false;

async function initVpnPanel(body) {
  const select = body.querySelector(".vpn-server-select");
  const serverButton = body.querySelector(".vpn-server-button");
  const serverMenu = body.querySelector(".vpn-server-menu");
  const toggleInput = body.querySelector(".vpn-toggle-input");
  const autoToggleInput = body.querySelector(".vpn-auto-toggle-input");
  if (!select || !toggleInput) return;

  logFrontend("vpn panel init");

  const vpnPanel = body.querySelector(".dash-vpn");
  const toggleShell = body.querySelector(".vpn-toggle:not(.vpn-auto-toggle)");
  vpnPanel?.addEventListener("click", (e) => e.stopPropagation());

  async function runVpnToggle(nextChecked, source) {
    logFrontend(`vpn toggle ${source} checked=${nextChecked} busy=${vpnBusy} selectDisabled=${select.disabled} tag=${select.value || ""}`);
    if (vpnBusy) {
      logFrontend("vpn toggle ignored: busy");
      return;
    }
    vpnBusy = true;
    toggleInput.disabled = true;
    toggleInput.checked = nextChecked;
    try {
      if (nextChecked) {
        const tag = select.disabled ? null : (select.value || null);
        logFrontend(`vpn_connect invoke tag=${tag || "<auto>"}`);
        await invoke("vpn_connect", { tag });
        logFrontend("vpn_connect invoke OK");
      } else {
        logFrontend("vpn_disconnect invoke");
        await invoke("vpn_disconnect");
        logFrontend("vpn_disconnect invoke OK");
      }
    } catch (e) {
      logFrontend(`vpn toggle error=${e.message || e}`);
      showDebugMessage(`VPN: ${e.message || e}`);
      toggleInput.checked = !nextChecked;
    } finally {
      toggleInput.disabled = false;
      vpnBusy = false;
      refreshVpnStatus();
    }
  }

  toggleShell?.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    runVpnToggle(!toggleInput.checked, "click");
  });

  toggleInput.addEventListener("change", (e) => {
    e.stopPropagation();
    runVpnToggle(toggleInput.checked, "change");
  });

  async function loadVpnServers() {
    if (serverButton) {
      serverButton.textContent = "Loading...";
      serverButton.disabled = true;
      serverButton.dataset.error = "";
    }

    if (!vpnServersCache) {
      logFrontend("vpn_list_servers invoke");
      vpnServersCache = await invoke("vpn_list_servers");
      logFrontend(`vpn_list_servers OK count=${vpnServersCache.length}`);
    }
    select.innerHTML = vpnServersCache
      .map((s) => `<option value="${escapeHtml(s.tag)}">${escapeHtml(displayText(s.tag))}</option>`)
      .join("");
    select.disabled = false;
    renderVpnServerPicker(select, serverButton, serverMenu);
    if (serverButton) {
      serverButton.disabled = false;
      serverButton.dataset.error = "";
    }
  }

  try {
    await loadVpnServers();
  } catch (e) {
    logFrontend(`vpn_list_servers error=${e.message || e}`);
    select.innerHTML = `<option>Subscription error</option>`;
    select.disabled = true;
    if (serverButton) {
      serverButton.textContent = "Subscription error · retry";
      serverButton.disabled = false;
      serverButton.dataset.error = "1";
    }
  }

  async function connectTo(tag) {
    if (vpnBusy) return;
    vpnBusy = true;
    toggleInput.disabled = true;
    select.disabled = true;
    if (serverButton) serverButton.disabled = true;
    try {
      select.value = tag;
      renderVpnServerPicker(select, serverButton, serverMenu);
      logFrontend(`vpn_connect (picker) tag=${tag}`);
      await invoke("vpn_connect", { tag });
    } catch (e) {
      showDebugMessage(`VPN: ${e.message || e}`);
    } finally {
      toggleInput.disabled = false;
      select.disabled = false;
      if (serverButton) serverButton.disabled = false;
      vpnBusy = false;
      refreshVpnStatus();
    }
  }

  async function pingAllServers() {
    if (vpnPinging) return;
    const btn = serverMenu?.querySelector(".vpn-ping-btn");
    vpnPinging = true;
    if (btn) {
      btn.disabled = true;
      btn.classList.add("busy");
      btn.textContent = "PINGING…";
    }
    try {
      logFrontend("vpn_ping_all invoke");
      const results = await invoke("vpn_ping_all");
      logFrontend(`vpn_ping_all OK count=${results.length}`);
      vpnPingResults = new Map(results.map((r) => [r.tag, r]));
      renderVpnServerPicker(select, serverButton, serverMenu);
      const working = results
        .filter((r) => r.ok && Number.isFinite(r.latencyMs))
        .sort((a, b) => a.latencyMs - b.latencyMs);
      if (working.length) {
        const best = working[0];
        showDebugMessage(`⚡ ${displayText(best.tag)} · ${best.latencyMs} ms`);
        await connectTo(best.tag);
      } else {
        showDebugMessage("No working servers found");
      }
    } catch (e) {
      logFrontend(`vpn_ping_all error=${e.message || e}`);
      showDebugMessage(`Ping: ${e.message || e}`);
    } finally {
      vpnPinging = false;
      if (btn) {
        btn.disabled = false;
        btn.classList.remove("busy");
        btn.textContent = "PING ALL";
      }
    }
  }

  serverButton?.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (vpnBusy) return;
    if (serverButton.dataset.error || !vpnServersCache?.length) {
      loadVpnServers().catch((error) => {
        logFrontend(`vpn_list_servers retry error=${error.message || error}`);
        serverButton.textContent = "Subscription error · retry";
        serverButton.disabled = false;
        serverButton.dataset.error = "1";
      });
      return;
    }
    if (select.disabled) return;
    if (surfaceManager.isOpen(serverMenu) || serverMenu.classList.contains("open")) {
      surfaceManager.closeSurface(serverMenu);
    } else {
      renderVpnServerPicker(select, serverButton, serverMenu);
      surfaceManager.openSurface(serverMenu, {
        origin: serverButton,
        originRadius: "10px",
        elevation: "var(--z-surface-menu, 4000)",
        display: "flex",
        ignoreOutsideElements: [serverButton],
        onOpen: () => {
          serverMenu.classList.add("open");
        },
        onClosed: () => {
          serverMenu.classList.remove("open");
        },
      });
    }
  });

  serverMenu?.addEventListener("click", (e) => {
    if (e.target.closest(".vpn-ping-btn")) {
      e.preventDefault();
      e.stopPropagation();
      pingAllServers();
      return;
    }
    const item = e.target.closest(".vpn-server-option");
    if (!item) return;
    e.preventDefault();
    e.stopPropagation();
    surfaceManager.closeSurface(serverMenu);
    connectTo(item.dataset.tag || "");
  });

  autoToggleInput?.addEventListener("change", async () => {
    const nextChecked = autoToggleInput.checked;
    isUpdatingAutoConnect = true;
    autoToggleInput.disabled = true;
    try {
      logFrontend(`vpn_set_auto_connect invoke enabled=${nextChecked}`);
      await invoke("vpn_set_auto_connect", { enabled: nextChecked });
    } catch (e) {
      logFrontend(`vpn_set_auto_connect error=${e.message || e}`);
      showDebugMessage(`VPN: ${e.message || e}`);
      autoToggleInput.checked = !nextChecked;
    } finally {
      autoToggleInput.disabled = false;
      isUpdatingAutoConnect = false;
      refreshVpnStatus();
    }
  });

  refreshVpnStatus();

  if (!window.vpnStatusInterval) {
    window.vpnStatusInterval = setInterval(refreshVpnStatus, 3000);
  }
}

async function refreshVpnStatus() {
  let status;
  try {
    status = await invoke("vpn_status");
  } catch {
    return;
  }

  document.querySelectorAll(".dash-vpn").forEach((panel) => {
    const icon = panel.querySelector(".vpn-icon");
    const label = panel.querySelector(".vpn-label");
    const serverName = panel.querySelector(".vpn-server-name");
    const toggleInput = panel.querySelector(".vpn-toggle-input");
    const select = panel.querySelector(".vpn-server-select");
    const serverButton = panel.querySelector(".vpn-server-button");
    const serverMenu = panel.querySelector(".vpn-server-menu");
    const autoToggleInput = panel.querySelector(".vpn-auto-toggle-input");

    if (icon) icon.classList.toggle("on", status.connected);
    if (icon) icon.classList.toggle("off", !status.connected);
    if (label) {
      label.textContent = status.connecting
        ? "VPN: Подключение..."
        : status.connected
        ? "VPN: On"
        : "VPN: Off";
    }
    if (serverName) serverName.textContent = formatSelectedVpnServer(status);
    if (toggleInput && !vpnBusy) toggleInput.checked = status.connected;
    if (select && status.selected_tag && select.value !== status.selected_tag && !vpnBusy) {
      select.value = status.selected_tag;
      renderVpnServerPicker(select, serverButton, serverMenu);
    }
    if (autoToggleInput && !isUpdatingAutoConnect && document.activeElement !== autoToggleInput) {
      const autoVal = status.auto_connect ?? status.autoConnect ?? false;
      autoToggleInput.checked = Boolean(autoVal);
    }
  });
}

function renderVpnServerPicker(select, button, menu) {
  if (!select || !button || !menu || !vpnServersCache?.length) return;
  const list = menu.querySelector(".vpn-menu-list") || menu;
  const selected = vpnServersCache.find((server) => server.tag === select.value) || vpnServersCache[0];
  if (selected && !select.value) select.value = selected.tag;

  button.textContent = selected ? displayText(selected.tag) : "Select server";
  button.disabled = select.disabled;

  const rank = (server) => {
    const r = vpnPingResults?.get(server.tag);
    if (r && r.ok && Number.isFinite(r.latencyMs)) return [0, r.latencyMs];
    if (r) return [1, 0];
    return [2, 0];
  };
  const ordered = vpnPingResults
    ? [...vpnServersCache].sort((a, b) => {
        const ra = rank(a);
        const rb = rank(b);
        return ra[0] - rb[0] || ra[1] - rb[1];
      })
    : vpnServersCache;

  list.innerHTML = ordered
    .map((server, i) => {
      const r = vpnPingResults?.get(server.tag);
      let badge = `<span class="vpn-opt-ping ping-unknown"></span>`;
      if (r && r.ok && Number.isFinite(r.latencyMs)) {
        const cls = r.latencyMs < 300 ? "ping-good" : r.latencyMs < 700 ? "ping-mid" : "ping-slow";
        badge = `<span class="vpn-opt-ping ${cls}">${r.latencyMs}<i>ms</i></span>`;
      } else if (r) {
        badge = `<span class="vpn-opt-ping ping-fail">fail</span>`;
      }
      return `
      <button type="button"
        class="vpn-server-option${server.tag === select.value ? " selected" : ""}"
        data-tag="${escapeHtml(server.tag)}" style="--i:${i}">
        <span class="vpn-opt-name">${escapeHtml(displayText(server.tag))}</span>
        ${badge}
      </button>`;
    })
    .join("");
}

function formatSelectedVpnServer(status) {
  const tag = status?.selected_tag;
  if (!tag) return "—";
  const name = displayText(tag);
  let line = name;
  if (Number.isFinite(status.proxyLatencyMs)) {
    line = `${name} · proxy: ${status.proxyLatencyMs} ms`;
  } else if (status.proxyError) {
    line = `${name} · proxy: fail`;
  } else if (status.connecting) {
    line = `${name} · proxy: ...`;
  }
  if (status.connected && status.ipUnverified) {
    line = `${line} · ip: ?`;
  }
  return line;
}

export { initVpnPanel, refreshVpnStatus };
