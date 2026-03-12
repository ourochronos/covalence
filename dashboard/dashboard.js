// Covalence Dashboard — Phase 1: Stats & Observability
//
// Fetches data from the Covalence API and renders it into the
// dashboard cards. All API calls go through apiFetch() which
// handles auth headers and error display.

const API_BASE = "/api/v1";

// If an API key is set, include it in requests.
// In dev mode (no key configured), the server allows all requests.
let apiKey = null;

async function apiFetch(path) {
  const opts = { headers: {} };
  if (apiKey) {
    opts.headers["Authorization"] = `Bearer ${apiKey}`;
  }
  const res = await fetch(`${API_BASE}${path}`, opts);
  if (!res.ok) {
    throw new Error(`${res.status} ${res.statusText}`);
  }
  // 204 No Content returns null
  if (res.status === 204) return null;
  return res.json();
}

function fmt(n) {
  if (typeof n !== "number") return "--";
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "k";
  return n.toLocaleString();
}

function shortId(id) {
  return id ? id.substring(0, 8) : "--";
}

function relativeTime(iso) {
  if (!iso) return "--";
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

// --- Data fetchers ---

async function fetchHealth() {
  const badge = document.getElementById("health-badge");
  try {
    const data = await apiFetch("/admin/health");
    badge.textContent = data.status;
    badge.className = "badge ok";
  } catch {
    badge.textContent = "offline";
    badge.className = "badge error";
  }
}

async function fetchGraphStats() {
  try {
    const data = await apiFetch("/graph/stats");
    document.getElementById("node-count").textContent = fmt(data.node_count);
    document.getElementById("edge-count").textContent = fmt(data.edge_count);
    document.getElementById("density").textContent =
      data.density.toFixed(4);
    document.getElementById("component-count").textContent =
      fmt(data.component_count);
  } catch (e) {
    document.getElementById("node-count").textContent = "err";
  }
}

async function fetchSources() {
  try {
    const sources = await apiFetch("/sources?limit=200");
    document.getElementById("source-count").textContent = fmt(sources.length);

    // Count unique source types
    const types = new Set(sources.map((s) => s.source_type));
    document.getElementById("source-types").textContent = types.size;

    // Recent sources table (last 8)
    const recent = sources.slice(0, 8);
    const container = document.getElementById("recent-sources");
    if (recent.length === 0) {
      container.innerHTML = '<span class="dim">No sources</span>';
      return;
    }
    let html = `<table>
      <thead><tr>
        <th>ID</th><th>Type</th><th>Title</th><th>Ingested</th>
      </tr></thead><tbody>`;
    for (const s of recent) {
      const title = s.title
        ? s.title.length > 50
          ? s.title.substring(0, 50) + "..."
          : s.title
        : "--";
      html += `<tr>
        <td class="mono">${shortId(s.id)}</td>
        <td>${s.source_type}</td>
        <td>${escapeHtml(title)}</td>
        <td class="dim">${relativeTime(s.ingested_at)}</td>
      </tr>`;
    }
    html += "</tbody></table>";
    container.innerHTML = html;
  } catch {
    document.getElementById("source-count").textContent = "err";
  }
}

async function fetchCommunities() {
  try {
    const communities = await apiFetch("/graph/communities?min_size=2");
    document.getElementById("community-count").textContent = fmt(
      communities.length
    );

    if (communities.length > 0) {
      const largest = Math.max(...communities.map((c) => c.size));
      document.getElementById("largest-community").textContent = fmt(largest);
    }

    // Show top 8 communities
    const container = document.getElementById("community-list");
    const top = communities.sort((a, b) => b.size - a.size).slice(0, 8);
    let html = "";
    for (const c of top) {
      const label = c.label || `Community ${c.id}`;
      html += `<div class="list-item">
        <span class="name">${escapeHtml(label)}</span>
        <span class="meta">${c.size} nodes | coherence ${c.coherence.toFixed(2)} | k-core ${c.core_level}</span>
      </div>`;
    }
    container.innerHTML = html || '<span class="dim">No communities</span>';
  } catch {
    document.getElementById("community-count").textContent = "err";
  }
}

async function fetchTraces() {
  try {
    const traces = await apiFetch("/admin/traces?limit=20");
    document.getElementById("trace-count").textContent = fmt(traces.length);

    if (traces.length > 0) {
      const avgMs =
        traces.reduce((sum, t) => sum + t.execution_ms, 0) / traces.length;
      document.getElementById("avg-execution-ms").textContent =
        Math.round(avgMs);
    }

    // Show recent traces
    const container = document.getElementById("recent-traces");
    const recent = traces.slice(0, 8);
    if (recent.length === 0) {
      container.innerHTML = '<span class="dim">No search traces</span>';
      return;
    }
    let html = `<table>
      <thead><tr>
        <th>Query</th><th>Strategy</th><th>Results</th><th>Time</th><th>When</th>
      </tr></thead><tbody>`;
    for (const t of recent) {
      const query =
        t.query_text.length > 40
          ? t.query_text.substring(0, 40) + "..."
          : t.query_text;
      html += `<tr>
        <td>${escapeHtml(query)}</td>
        <td class="mono">${t.strategy}</td>
        <td>${t.result_count}</td>
        <td class="mono">${t.execution_ms}ms</td>
        <td class="dim">${relativeTime(t.created_at)}</td>
      </tr>`;
    }
    html += "</tbody></table>";
    container.innerHTML = html;
  } catch {
    document.getElementById("trace-count").textContent = "err";
  }
}

async function fetchGaps() {
  try {
    const data = await apiFetch("/admin/knowledge-gaps?limit=10");
    document.getElementById("gap-count").textContent = fmt(data.gap_count);

    const container = document.getElementById("gap-list");
    if (data.gaps.length === 0) {
      container.innerHTML = '<span class="dim">No knowledge gaps</span>';
      return;
    }
    let html = "";
    for (const g of data.gaps.slice(0, 8)) {
      html += `<div class="list-item">
        <span class="name">${escapeHtml(g.canonical_name)} <span class="dim">(${g.node_type})</span></span>
        <span class="meta">in:${g.in_degree} out:${g.out_degree} gap:${g.gap_score.toFixed(1)}</span>
      </div>`;
    }
    container.innerHTML = html;
  } catch {
    document.getElementById("gap-count").textContent = "err";
  }
}

async function fetchMemory() {
  try {
    const data = await apiFetch("/memory/status");
    document.getElementById("memory-count").textContent = fmt(
      data.total_memories
    );
    document.getElementById("entity-count").textContent = fmt(
      data.total_entities
    );
    document.getElementById("relationship-count").textContent = fmt(
      data.total_relationships
    );
  } catch {
    document.getElementById("memory-count").textContent = "err";
  }
}

async function fetchTopology() {
  try {
    const data = await apiFetch("/graph/topology");
    const container = document.getElementById("topology-map");

    if (!data.domains || data.domains.length === 0) {
      container.innerHTML = '<span class="dim">No topology data</span>';
      return;
    }

    // Sort by node count descending
    const domains = data.domains.sort((a, b) => b.node_count - a.node_count);
    let html = "";
    for (const d of domains.slice(0, 12)) {
      const label = d.label || `Domain ${d.community_id}`;
      html += `<div class="domain-card">
        <div class="domain-label">${escapeHtml(label)}</div>
        <div class="domain-meta">
          ${d.node_count} nodes | coherence ${d.coherence.toFixed(2)} | PR ${d.avg_pagerank.toFixed(4)}
        </div>
      </div>`;
    }
    if (domains.length > 12) {
      html += `<div class="domain-card">
        <div class="domain-label">+${domains.length - 12} more</div>
        <div class="domain-meta">${data.links.length} inter-domain links</div>
      </div>`;
    }
    container.innerHTML = html;
  } catch {
    document.getElementById("topology-map").innerHTML =
      '<span class="error-text">Failed to load topology</span>';
  }
}

function escapeHtml(str) {
  const div = document.createElement("div");
  div.textContent = str;
  return div.innerHTML;
}

async function refreshAll() {
  // Fire all fetches in parallel
  await Promise.allSettled([
    fetchHealth(),
    fetchGraphStats(),
    fetchSources(),
    fetchCommunities(),
    fetchTraces(),
    fetchGaps(),
    fetchMemory(),
    fetchTopology(),
  ]);
  document.getElementById("last-refresh").textContent =
    `Last refresh: ${new Date().toLocaleTimeString()}`;
}

// Initial load
refreshAll();

// Auto-refresh every 30 seconds
setInterval(refreshAll, 30000);
