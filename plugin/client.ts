/**
 * HTTP client for the Covalence REST API.
 * Replaces CLI exec with direct fetch calls to localhost:8430.
 */

import type { CovalenceConfig } from "./config.js";

export interface CovalenceResult {
  success: boolean;
  data?: any;
  error?: string;
  status?: number;
}

/**
 * Core HTTP fetch helper for the Covalence API.
 */
async function covalenceFetch(
  cfg: CovalenceConfig,
  method: string,
  path: string,
  body?: unknown,
  options?: { timeout?: number; query?: Record<string, string | undefined> },
): Promise<CovalenceResult> {
  let url = `${cfg.serverUrl}${path}`;

  // Append query params if any
  if (options?.query) {
    const params = new URLSearchParams();
    for (const [key, val] of Object.entries(options.query)) {
      if (val !== undefined) params.set(key, val);
    }
    const qs = params.toString();
    if (qs) url += `?${qs}`;
  }

  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    Accept: "application/json",
  };

  if (cfg.authToken) {
    headers["Authorization"] = `Bearer ${cfg.authToken}`;
  }

  const controller = new AbortController();
  const timeoutMs = options?.timeout ?? 30000;
  const timer = setTimeout(() => controller.abort(), timeoutMs);

  try {
    const fetchOptions: RequestInit = {
      method,
      headers,
      signal: controller.signal,
    };

    if (body !== undefined && method !== "GET" && method !== "DELETE") {
      fetchOptions.body = JSON.stringify(body);
    }

    const response = await fetch(url, fetchOptions);
    const text = await response.text();

    let data: unknown;
    try {
      data = text ? JSON.parse(text) : null;
    } catch {
      data = { text: text.trim() };
    }

    if (!response.ok) {
      const errorMsg =
        (data as any)?.error ||
        (data as any)?.message ||
        `HTTP ${response.status}: ${response.statusText}`;
      const errorMsgStr: string =
        typeof errorMsg === "string" ? errorMsg : JSON.stringify(errorMsg);
      return { success: false, data, error: errorMsgStr, status: response.status };
    }

    return { success: true, data, status: response.status };
  } catch (err: any) {
    if (err.name === "AbortError") {
      return { success: false, error: `Request timed out after ${timeoutMs}ms` };
    }
    return { success: false, error: err?.message || String(err) };
  } finally {
    clearTimeout(timer);
  }
}

// =========================================================================
// Source API
// =========================================================================

export function ingestSource(
  cfg: CovalenceConfig,
  params: {
    content: string;
    source_type: string;
    title?: string;
    url?: string;
    metadata?: unknown;
  },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/sources", params);
}

export function getSource(cfg: CovalenceConfig, sourceId: string): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "GET", `/sources/${encodeURIComponent(sourceId)}`);
}

export function searchSources(
  cfg: CovalenceConfig,
  query: string,
  limit?: number,
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "GET", "/sources", undefined, {
    query: { q: query, limit: limit !== undefined ? String(limit) : undefined },
  });
}

export function deleteSource(cfg: CovalenceConfig, sourceId: string): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "DELETE", `/sources/${encodeURIComponent(sourceId)}`);
}

// =========================================================================
// Article API
// =========================================================================

export function createArticle(
  cfg: CovalenceConfig,
  params: {
    content: string;
    title?: string;
    source_ids?: string[];
    author_type?: string;
    domain_path?: string[];
  },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/articles", params);
}

export function compileArticle(
  cfg: CovalenceConfig,
  params: { source_ids: string[]; title_hint?: string },
  options?: { timeout?: number },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/articles/compile", params, options);
}

export function mergeArticles(
  cfg: CovalenceConfig,
  params: { article_id_a: string; article_id_b: string },
  options?: { timeout?: number },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/articles/merge", params, options);
}

export function getArticle(
  cfg: CovalenceConfig,
  articleId: string,
  includeProvenance?: boolean,
): Promise<CovalenceResult> {
  return covalenceFetch(
    cfg,
    "GET",
    `/articles/${encodeURIComponent(articleId)}${includeProvenance ? "/provenance" : ""}`,
  );
}

export function updateArticle(
  cfg: CovalenceConfig,
  articleId: string,
  params: { content: string; source_id?: string },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "PATCH", `/articles/${encodeURIComponent(articleId)}`, params);
}

export function splitArticle(cfg: CovalenceConfig, articleId: string): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", `/articles/${encodeURIComponent(articleId)}/split`);
}

export function deleteArticle(cfg: CovalenceConfig, articleId: string): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "DELETE", `/articles/${encodeURIComponent(articleId)}`);
}

// =========================================================================
// Search API
// =========================================================================

export function searchKnowledge(
  cfg: CovalenceConfig,
  params: {
    query: string;
    limit?: number;
    include_sources?: boolean;
    session_id?: string;
    weights?: Record<string, number>;
    intent?: string;
    mode?: string;
    strategy?: string;
    recency_bias?: number;
    domain_path?: string[];
    max_hops?: number;
  },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/search", params);
}

// =========================================================================
// Provenance API
// =========================================================================

export function traceProvenance(
  cfg: CovalenceConfig,
  articleId: string,
  claimText: string,
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "GET", `/articles/${encodeURIComponent(articleId)}/provenance`, undefined, {
    query: { claim: claimText },
  });
}

// =========================================================================
// Contention API
// =========================================================================

export function listContentions(
  cfg: CovalenceConfig,
  params?: { node_id?: string; status?: string },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "GET", "/contentions", undefined, {
    query: {
      node_id: params?.node_id,
      status: params?.status,
    },
  });
}

export function resolveContention(
  cfg: CovalenceConfig,
  contentionId: string,
  params: { resolution: string; rationale: string },
): Promise<CovalenceResult> {
  return covalenceFetch(
    cfg,
    "POST",
    `/contentions/${encodeURIComponent(contentionId)}/resolve`,
    params,
  );
}

// =========================================================================
// Memory API
// =========================================================================

export function storeMemory(
  cfg: CovalenceConfig,
  params: {
    content: string;
    context?: string;
    importance?: number;
    tags?: string[];
    supersedes_id?: string;
  },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/memory", params);
}

export function recallMemory(
  cfg: CovalenceConfig,
  params: {
    query: string;
    limit?: number;
    min_confidence?: number;
    tags?: string[];
  },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/memory/search", params);
}

export function memoryStatus(cfg: CovalenceConfig): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "GET", "/memory/status");
}

export function forgetMemory(
  cfg: CovalenceConfig,
  memoryId: string,
  reason?: string,
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "PATCH", `/memory/${encodeURIComponent(memoryId)}/forget`, { reason });
}

// =========================================================================
// Session API
// =========================================================================

export function createSession(
  cfg: CovalenceConfig,
  params: { session_id?: string; platform?: string; channel?: string; parent_session_id?: string },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/sessions", params);
}

export function closeSession(cfg: CovalenceConfig, sessionId: string): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", `/sessions/${encodeURIComponent(sessionId)}/close`);
}

export function appendMessages(
  cfg: CovalenceConfig,
  sessionId: string,
  messages: Array<{ role: string; content: string; speaker?: string; chunk_index?: number }>,
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", `/sessions/${encodeURIComponent(sessionId)}/messages`, {
    messages,
  });
}

export function flushSession(cfg: CovalenceConfig, sessionId: string): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", `/sessions/${encodeURIComponent(sessionId)}/flush`);
}

export function finalizeSession(
  cfg: CovalenceConfig,
  sessionId: string,
  compile?: boolean,
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", `/sessions/${encodeURIComponent(sessionId)}/finalize`, {
    compile: compile ?? false,
  });
}

// =========================================================================
// Admin API
// =========================================================================

export function getAdminStats(cfg: CovalenceConfig): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "GET", "/admin/stats");
}

export function runMaintenance(
  cfg: CovalenceConfig,
  params: {
    recompute_scores?: boolean;
    process_queue?: boolean;
    evict_if_over_capacity?: boolean;
    evict_count?: number;
  },
  options?: { timeout?: number },
): Promise<CovalenceResult> {
  return covalenceFetch(cfg, "POST", "/admin/maintenance", params, options);
}

// =========================================================================
// Health Check
// =========================================================================

export async function healthCheck(
  cfg: CovalenceConfig,
): Promise<{ ok: boolean; version?: string; database?: string; error?: string }> {
  const result = await covalenceFetch(cfg, "GET", "/admin/stats", undefined, { timeout: 5000 });
  if (result.success && result.data) {
    return {
      ok: true,
      version: result.data.version || result.data.engine_version || "unknown",
      database: result.data.database || result.data.db_name || "covalence",
    };
  }
  return { ok: false, error: result.error };
}
