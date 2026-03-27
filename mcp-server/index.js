#!/usr/bin/env node
/**
 * Covalence MCP Server
 *
 * Bridges Claude Code to Covalence's knowledge engine via the MCP protocol.
 * Runs as a stdio server — Claude Code spawns it and communicates via JSON-RPC.
 *
 * Tools exposed (10):
 *   - covalence_search: Multi-dimensional fused search
 *   - covalence_ask: LLM-powered knowledge synthesis
 *   - covalence_health: System health report
 *   - covalence_data_health: Data hygiene preview
 *   - covalence_alignment: Cross-domain alignment analysis
 *   - covalence_node: Get node details
 *   - covalence_blast_radius: Impact analysis for a code entity
 *   - covalence_memory_store: Store a memory in the knowledge graph
 *   - covalence_memory_recall: Recall memories by semantic query
 *   - covalence_memory_forget: Forget a memory by ID
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

const API_URL = process.env.COVALENCE_API_URL || "http://covalence-wsl:8441";

async function apiCall(path, method = "GET", body = null) {
  const opts = {
    method,
    headers: { "Content-Type": "application/json" },
  };
  if (body) opts.body = JSON.stringify(body);

  const resp = await fetch(`${API_URL}/api/v1${path}`, opts);
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`API error ${resp.status}: ${text}`);
  }
  return resp.json();
}

const server = new McpServer({
  name: "covalence",
  version: "0.1.0",
});

// --- Search ---
server.tool(
  "covalence_search",
  "Search the Covalence knowledge graph across all dimensions (vector, lexical, temporal, graph, structural, global). Returns ranked results from code, specs, research, and design docs.",
  {
    query: z.string().describe("Search query text"),
    limit: z.number().optional().describe("Max results (default 10)"),
    strategy: z.string().optional().describe("Search strategy: auto, balanced, precise, exploratory, recent, graph_first, global"),
    graph_view: z.string().optional().describe("Orthogonal graph view: causal, temporal, entity, structural, all"),
  },
  async ({ query, limit, strategy, graph_view }) => {
    const body = { query, limit: limit || 10 };
    if (strategy) body.strategy = strategy;
    if (graph_view) body.graph_view = graph_view;
    const results = await apiCall("/search", "POST", body);
    return {
      content: [{ type: "text", text: JSON.stringify(results, null, 2) }],
    };
  }
);

// --- Ask (LLM synthesis) ---
server.tool(
  "covalence_ask",
  "Ask a question and get an LLM-synthesized answer grounded in the knowledge graph with citations. Uses Sonnet by default for deep reasoning.",
  {
    question: z.string().describe("The question to answer"),
    strategy: z.string().optional().describe("Search strategy for context retrieval"),
    model: z.string().optional().describe("LLM model override: haiku, sonnet, opus, gemini"),
  },
  async ({ question, strategy, model }) => {
    const body = { question };
    if (strategy) body.strategy = strategy;
    if (model) body.model = model;
    const result = await apiCall("/ask", "POST", body);
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  }
);

// --- Health Report ---
server.tool(
  "covalence_health",
  "Get a comprehensive health report: graph stats, source counts by domain, entity class distribution, pipeline progress (summary percentages), and queue status.",
  {},
  async () => {
    const report = await apiCall("/admin/health-report");
    return {
      content: [{ type: "text", text: JSON.stringify(report, null, 2) }],
    };
  }
);

// --- Data Health ---
server.tool(
  "covalence_data_health",
  "Preview data hygiene: superseded sources, orphan nodes, duplicates, unembedded/unsummarized entities. Read-only — shows what could be cleaned without modifying anything.",
  {},
  async () => {
    const report = await apiCall("/admin/data-health");
    return {
      content: [{ type: "text", text: JSON.stringify(report, null, 2) }],
    };
  }
);

// --- Alignment Report ---
server.tool(
  "covalence_alignment",
  "Run cross-domain alignment analysis: code ahead of spec, spec ahead of code, design contradicted by research, stale design docs.",
  {
    checks: z.array(z.string()).optional().describe("Which checks: code_ahead, spec_ahead, design_contradicted, stale_design. Empty = all."),
    limit: z.number().optional().describe("Max items per check (default 10)"),
  },
  async ({ checks, limit }) => {
    const body = { limit: limit || 10 };
    if (checks && checks.length > 0) body.checks = checks;
    const report = await apiCall("/analysis/alignment", "POST", body);
    return {
      content: [{ type: "text", text: JSON.stringify(report, null, 2) }],
    };
  }
);

// --- Node Details ---
server.tool(
  "covalence_node",
  "Get details about a specific node in the knowledge graph by name. Returns entity class, type, description, domain entropy, and primary domain.",
  {
    name: z.string().describe("Node canonical name to look up"),
  },
  async ({ name }) => {
    // Use search to find the node, then get details
    const results = await apiCall("/search", "POST", {
      query: name,
      limit: 1,
      node_types: ["concept", "technology", "function", "struct", "trait", "component"],
    });
    const items = Array.isArray(results) ? results : results.results || results.data || [];
    if (items.length === 0) {
      return { content: [{ type: "text", text: `No node found matching "${name}"` }] };
    }
    const nodeId = items[0].id;
    const node = await apiCall(`/nodes/${nodeId}?explain=true`);
    return {
      content: [{ type: "text", text: JSON.stringify(node, null, 2) }],
    };
  }
);

// --- Blast Radius ---
server.tool(
  "covalence_blast_radius",
  "Analyze the impact of changing a code entity. Shows directly affected nodes, component impact, and cascading functions.",
  {
    target: z.string().describe("Entity name (e.g., 'PgResolver', 'SearchService')"),
    max_hops: z.number().optional().describe("Max traversal depth (default 2)"),
    include_invalidated: z.boolean().optional().describe("Include invalidated edges (default false)"),
  },
  async ({ target, max_hops, include_invalidated }) => {
    const body = { target };
    if (max_hops) body.max_hops = max_hops;
    if (include_invalidated) body.include_invalidated = include_invalidated;
    const result = await apiCall("/analysis/blast-radius", "POST", body);
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  }
);

// --- Memory Store ---
server.tool(
  "covalence_memory_store",
  "Store a memory in the knowledge graph with optional agent identity, topic, and confidence.",
  {
    content: z.string().describe("The memory content to store"),
    topic: z.string().optional().describe("Topic tag for the memory"),
    agent_id: z.string().optional().describe("Agent identifier for scoped recall"),
    confidence: z.number().optional().describe("Confidence level 0.0-1.0"),
  },
  async ({ content, topic, agent_id, confidence }) => {
    const body = { content };
    if (topic) body.topic = topic;
    if (agent_id) body.agent_id = agent_id;
    if (confidence !== undefined) body.confidence = confidence;
    const result = await apiCall("/memory", "POST", body);
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  }
);

// --- Memory Recall ---
server.tool(
  "covalence_memory_recall",
  "Recall memories from the knowledge graph by semantic query, optionally scoped to an agent.",
  {
    query: z.string().describe("Search query for memory recall"),
    limit: z.number().optional().describe("Max results (default 10)"),
    agent_id: z.string().optional().describe("Filter to specific agent's memories"),
    topic: z.string().optional().describe("Filter by topic"),
  },
  async ({ query, limit, agent_id, topic }) => {
    const body = { query, limit: limit || 10 };
    if (agent_id) body.agent_id = agent_id;
    if (topic) body.topic = topic;
    const result = await apiCall("/memory/recall", "POST", body);
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  }
);

// --- Memory Forget ---
server.tool(
  "covalence_memory_forget",
  "Forget (delete) a specific memory by ID.",
  {
    id: z.string().describe("Memory ID to forget"),
  },
  async ({ id }) => {
    const result = await apiCall(`/memory/${id}`, "DELETE");
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  }
);

// Start the server
const transport = new StdioServerTransport();
await server.connect(transport);
