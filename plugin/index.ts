/**
 * OpenClaw Memory Plugin: Covalence Knowledge Graph Engine (HTTP backend)
 *
 * Exposes Covalence v1 tools via REST API for OpenClaw agents.
 * Provides: source ingestion, article management, knowledge search,
 * contention resolution, admin tools, memory wrappers, and session lifecycle.
 */

import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import { Type } from "@sinclair/typebox";
import {
  healthCheck,
  ingestSource,
  getSource,
  searchSources,
  deleteSource,
  createArticle,
  compileArticle,
  mergeArticles,
  getArticle,
  updateArticle,
  splitArticle,
  deleteArticle,
  searchKnowledge,
  traceProvenance,
  listContentions,
  resolveContention,
  storeMemory,
  recallMemory,
  memoryStatus,
  forgetMemory,
  createSession,
  closeSession,
  getAdminStats,
  runMaintenance,
} from "./client.js";
import { covalenceConfigSchema } from "./config.js";
import { registerInferenceEndpoints } from "./inference.js";

// =========================================================================
// Tool Helpers
// =========================================================================

function stringEnum<T extends string>(values: readonly T[], opts?: { description?: string }) {
  return Type.Unsafe<T>({ type: "string", enum: [...values], ...opts });
}

function ok(data: unknown) {
  return {
    content: [{ type: "text" as const, text: JSON.stringify(data, null, 2) }],
    details: data,
  };
}

// =========================================================================
// Capture Heuristics
// =========================================================================

const CAPTURE_TRIGGERS = [
  /remember|don't forget|keep in mind/i,
  /i prefer|i like|i want|i need|i hate/i,
  /we decided|decision:|chose to|going with/i,
  /my .+ is|is my/i,
  /always|never|important to note/i,
  /key takeaway|lesson learned|note to self/i,
];

function shouldCapture(text: string): boolean {
  if (text.length < 15 || text.length > 500) return false;
  if (text.includes("<relevant-knowledge>")) return false;
  if (text.startsWith("<") && text.includes("</")) return false;
  if (text.includes("```")) return false;
  return CAPTURE_TRIGGERS.some((r) => r.test(text));
}

// =========================================================================
// Plugin Definition
// =========================================================================

const covalencePlugin = {
  id: "memory-covalence",
  name: "Memory (Covalence)",
  description:
    "Covalence knowledge graph engine — sources, articles, contentions, memory (HTTP backend)",
  kind: "memory" as const,
  configSchema: covalenceConfigSchema,

  register(api: OpenClawPluginApi) {
    const cfg = covalenceConfigSchema.parse(api.pluginConfig);
    const log = api.logger;

    // =====================
    // TOOLS — 20 tools
    // =====================

    // =========================================================================
    // Source tools
    // =========================================================================

    // 1. source_ingest
    api.registerTool(
      {
        name: "source_ingest",
        label: "Ingest Source",
        description:
          "Ingest a new source into the knowledge substrate. " +
          "Sources are raw, immutable input material from which articles are compiled. " +
          "Call this whenever new information arrives — documents, conversations, web pages, code, observations.",
        parameters: Type.Object({
          content: Type.String({ description: "Raw text content of the source (required)" }),
          source_type: stringEnum(
            ["document", "conversation", "web", "code", "observation", "tool_output", "user_input"],
            {
              description:
                "Source type determines initial reliability score: " +
                "document/code=0.8, web=0.6, conversation=0.5, observation=0.4, tool_output=0.7, user_input=0.75",
            },
          ),
          title: Type.Optional(Type.String({ description: "Optional human-readable title" })),
          url: Type.Optional(Type.String({ description: "Optional canonical URL for web sources" })),
          metadata: Type.Optional(Type.Any({ description: "Optional arbitrary metadata (JSON object)" })),
        }),
        async execute(_id: string, params: Record<string, unknown>) {
          const result = await ingestSource(cfg, {
            content: String(params.content),
            source_type: String(params.source_type),
            title: params.title ? String(params.title) : undefined,
            url: params.url ? String(params.url) : undefined,
            metadata: params.metadata,
          });
          if (!result.success) throw new Error(result.error || "source_ingest failed");
          return ok(result.data);
        },
      },
      { name: "source_ingest" },
    );

    // 2. source_get
    api.registerTool(
      {
        name: "source_get",
        label: "Get Source",
        description: "Get a source by ID with full details including content and metadata.",
        parameters: Type.Object({
          source_id: Type.String({ description: "UUID of the source" }),
        }),
        async execute(_id: string, params: { source_id: string }) {
          const result = await getSource(cfg, params.source_id);
          if (!result.success) throw new Error(result.error || "source_get failed");
          return ok(result.data);
        },
      },
      { name: "source_get" },
    );

    // 3. source_search
    api.registerTool(
      {
        name: "source_search",
        label: "Search Sources",
        description:
          "Full-text search over source content. " +
          "Uses PostgreSQL full-text search. Results ordered by relevance descending.",
        parameters: Type.Object({
          query: Type.String({ description: "Search terms (natural language or keyword phrase)" }),
          limit: Type.Optional(
            Type.Number({ description: "Maximum results (default 20, max 200)" }),
          ),
        }),
        async execute(_id: string, params: { query: string; limit?: number }) {
          const result = await searchSources(cfg, params.query, params.limit);
          if (!result.success) throw new Error(result.error || "source_search failed");
          return ok(result.data);
        },
      },
      { name: "source_search" },
    );

    // =========================================================================
    // Knowledge search
    // =========================================================================

    // 4. knowledge_search
    api.registerTool(
      {
        name: "knowledge_search",
        label: "Search Knowledge",
        description:
          "Unified knowledge retrieval — search articles and optionally raw sources. " +
          "CRITICAL: Call this BEFORE answering questions about any topic that may have been " +
          "discussed, documented, or learned previously. This ensures responses are grounded in accumulated knowledge. " +
          "Supports intent-aware graph traversal, multiple search modes (standard/hierarchical/synthesis), " +
          "and search strategies (balanced/precise/exploratory/graph). " +
          "Results are ranked by: relevance × 0.5 + confidence × 0.35 + freshness × 0.15.",
        parameters: Type.Object({
          query: Type.String({ description: "Natural-language search query" }),
          limit: Type.Optional(
            Type.Number({ description: "Maximum results to return (default 10, max 200)" }),
          ),
          include_sources: Type.Optional(
            Type.Boolean({
              description: "Include ungrouped raw sources alongside compiled articles",
            }),
          ),
          session_id: Type.Optional(
            Type.String({ description: "Optional session ID for usage trace attribution" }),
          ),
          intent: Type.Optional(
            Type.String({
              description:
                "Search intent for graph traversal edge prioritization. " +
                "Values: factual (confirms/originates edges), temporal (valid_from/supersedes edges), " +
                "causal (causal edges), entity (involves/captured_in edges). " +
                "Omit for default behavior.",
            }),
          ),
          mode: Type.Optional(
            Type.String({
              description:
                "Search mode. Values: standard (flat search, default), " +
                "hierarchical (articles first, then expand linked sources via provenance), " +
                "synthesis (LLM-synthesized answer with inline citations from raw sources).",
            }),
          ),
          strategy: Type.Optional(
            Type.String({
              description:
                "Search strategy adjusts dimension weights. Values: balanced (default, vector=0.55 lexical=0.20 graph=0.10 structural=0.15), " +
                "precise (lexical-heavy for factual lookups, lexical=0.45), " +
                "exploratory (vector-heavy for conceptual queries, vector=0.65), " +
                "graph (graph-heavy for structural/relational queries, graph=0.40 + 2 hops default).",
            }),
          ),
          recency_bias: Type.Optional(
            Type.Number({
              description:
                "Recency bias factor (0.0-1.0). At 0.0 (default), freshness gets 10% weight. " +
                "At 1.0, freshness gets 40% weight (strongly favor recent content).",
            }),
          ),
          domain_path: Type.Optional(
            Type.Array(Type.String(), {
              description:
                "Domain path filter. Only nodes whose domain_path shares at least one element are returned.",
            }),
          ),
          max_hops: Type.Optional(
            Type.Number({
              description:
                "Maximum graph traversal hops (1-3, default 1). Higher values discover structurally distant but related nodes.",
            }),
          ),
        }),
        async execute(_id: string, params: Record<string, unknown>) {
          const result = await searchKnowledge(cfg, {
            query: String(params.query),
            limit: typeof params.limit === "number" ? params.limit : undefined,
            include_sources: typeof params.include_sources === "boolean" ? params.include_sources : undefined,
            session_id: params.session_id ? String(params.session_id) : undefined,
            intent: typeof params.intent === "string" ? params.intent : undefined,
            mode: typeof params.mode === "string" ? params.mode : undefined,
            strategy: typeof params.strategy === "string" ? params.strategy : undefined,
            recency_bias: typeof params.recency_bias === "number" ? params.recency_bias : undefined,
            domain_path: Array.isArray(params.domain_path) ? params.domain_path as string[] : undefined,
            max_hops: typeof params.max_hops === "number" ? params.max_hops : undefined,
          });
          if (!result.success) throw new Error(result.error || "knowledge_search failed");
          return ok(result.data);
        },
      },
      { name: "knowledge_search" },
    );

    // =========================================================================
    // Article tools
    // =========================================================================

    // 5. article_get
    api.registerTool(
      {
        name: "article_get",
        label: "Get Article",
        description:
          "Get an article by ID, optionally with its full provenance list. " +
          "Set include_provenance=true to see all linked sources and their relationship types " +
          "(originates, confirms, supersedes, contradicts, contends).",
        parameters: Type.Object({
          article_id: Type.String({ description: "UUID of the article" }),
          include_provenance: Type.Optional(
            Type.Boolean({ description: "Include linked source provenance in the response" }),
          ),
        }),
        async execute(_id: string, params: { article_id: string; include_provenance?: boolean }) {
          const result = await getArticle(cfg, params.article_id, params.include_provenance);
          if (!result.success) throw new Error(result.error || "article_get failed");
          return ok(result.data);
        },
      },
      { name: "article_get" },
    );

    // 6. article_create
    api.registerTool(
      {
        name: "article_create",
        label: "Create Article",
        description:
          "Manually create a new knowledge article. " +
          "Use this when you want to create an article directly without LLM compilation. " +
          "For compilation from sources, use article_compile instead. " +
          "Optionally link originating source UUIDs — they will be linked with relationship='originates'.",
        parameters: Type.Object({
          content: Type.String({ description: "Article body text (required)" }),
          title: Type.Optional(Type.String({ description: "Optional human-readable title" })),
          source_ids: Type.Optional(
            Type.Array(Type.String(), {
              description: "UUIDs of source documents this article originates from",
            }),
          ),
          author_type: Type.Optional(
            stringEnum(["system", "operator", "agent"], {
              description: "Who authored this article (default: system)",
            }),
          ),
          domain_path: Type.Optional(
            Type.Array(Type.String(), {
              description: "Hierarchical domain tags (e.g. ['python', 'stdlib'])",
            }),
          ),
        }),
        async execute(_id: string, params: Record<string, unknown>) {
          const result = await createArticle(cfg, {
            content: String(params.content),
            title: params.title ? String(params.title) : undefined,
            source_ids: Array.isArray(params.source_ids)
              ? params.source_ids.map(String)
              : undefined,
            author_type: params.author_type ? String(params.author_type) : undefined,
            domain_path: Array.isArray(params.domain_path)
              ? params.domain_path.map(String)
              : undefined,
          });
          if (!result.success) throw new Error(result.error || "article_create failed");
          return ok(result.data);
        },
      },
      { name: "article_create" },
    );

    // 7. article_compile
    api.registerTool(
      {
        name: "article_compile",
        label: "Compile Article",
        description:
          "Compile one or more sources into a new knowledge article using LLM summarization. " +
          "The LLM produces a coherent, right-sized article from the given source documents. " +
          "All sources are linked to the resulting article with appropriate provenance relationship types. " +
          "The compiled article respects right-sizing bounds (default: 200–4000 tokens, target 2000). " +
          "Returns 202 Accepted with a job_id — the compilation runs asynchronously.",
        parameters: Type.Object({
          source_ids: Type.Array(Type.String(), {
            description: "UUIDs of source documents to compile (required, non-empty)",
          }),
          title_hint: Type.Optional(
            Type.String({ description: "Optional hint for the article title" }),
          ),
        }),
        async execute(_id: string, params: { source_ids: string[]; title_hint?: string }) {
          const result = await compileArticle(
            cfg,
            { source_ids: params.source_ids, title_hint: params.title_hint },
            { timeout: 120000 },
          );
          // 202 Accepted is also a success for async compile
          if (!result.success && result.status !== 202) {
            throw new Error(result.error || "article_compile failed");
          }
          return ok(result.data);
        },
      },
      { name: "article_compile" },
    );

    // 8. article_update
    api.registerTool(
      {
        name: "article_update",
        label: "Update Article",
        description:
          "Update an article's content with new material. " +
          "Increments the article version, records an 'updated' mutation, and optionally links the triggering source. " +
          "The source is linked with a relationship type inferred from content (typically 'confirms' or 'supersedes').",
        parameters: Type.Object({
          article_id: Type.String({ description: "UUID of the article to update" }),
          content: Type.String({ description: "New article body text" }),
          source_id: Type.Optional(
            Type.String({ description: "Optional UUID of the source that triggered this update" }),
          ),
        }),
        async execute(
          _id: string,
          params: { article_id: string; content: string; source_id?: string },
        ) {
          const result = await updateArticle(cfg, params.article_id, {
            content: params.content,
            source_id: params.source_id,
          });
          if (!result.success) throw new Error(result.error || "article_update failed");
          return ok(result.data);
        },
      },
      { name: "article_update" },
    );

    // =========================================================================
    // Right-sizing tools
    // =========================================================================

    // 9. article_split
    api.registerTool(
      {
        name: "article_split",
        label: "Split Article",
        description:
          "Split an oversized article into two smaller articles. " +
          "The original article retains its ID and the first half of the content. " +
          "A new article is created for the remainder. Both inherit all provenance sources, " +
          "and mutation records of type 'split' are written for both.",
        parameters: Type.Object({
          article_id: Type.String({ description: "UUID of the article to split" }),
        }),
        async execute(_id: string, params: { article_id: string }) {
          const result = await splitArticle(cfg, params.article_id);
          if (!result.success) throw new Error(result.error || "article_split failed");
          return ok(result.data);
        },
      },
      { name: "article_split" },
    );

    // 10. article_merge
    api.registerTool(
      {
        name: "article_merge",
        label: "Merge Articles",
        description:
          "Merge two related articles into one. " +
          "A new article is created with combined content. Both originals are archived. " +
          "The merged article inherits the union of provenance sources from both. " +
          "Mutation records of type 'merged' are written.",
        parameters: Type.Object({
          article_id_a: Type.String({ description: "UUID of the first article" }),
          article_id_b: Type.String({ description: "UUID of the second article" }),
        }),
        async execute(
          _id: string,
          params: { article_id_a: string; article_id_b: string },
        ) {
          const result = await mergeArticles(
            cfg,
            { article_id_a: params.article_id_a, article_id_b: params.article_id_b },
            { timeout: 120000 },
          );
          if (!result.success) throw new Error(result.error || "article_merge failed");
          return ok(result.data);
        },
      },
      { name: "article_merge" },
    );

    // =========================================================================
    // Provenance
    // =========================================================================

    // 11. provenance_trace
    api.registerTool(
      {
        name: "provenance_trace",
        label: "Trace Provenance",
        description:
          "Trace which sources likely contributed a specific claim in an article. " +
          "Uses text-similarity (TF-IDF) to rank the article's linked sources by " +
          "how much their content overlaps with the given claim text. " +
          "Useful for attribution and fact-checking.",
        parameters: Type.Object({
          article_id: Type.String({ description: "UUID of the article" }),
          claim_text: Type.String({
            description: "The specific claim or sentence to trace back to sources",
          }),
        }),
        async execute(_id: string, params: { article_id: string; claim_text: string }) {
          const result = await traceProvenance(cfg, params.article_id, params.claim_text);
          if (!result.success) throw new Error(result.error || "provenance_trace failed");
          return ok(result.data);
        },
      },
      { name: "provenance_trace" },
    );

    // =========================================================================
    // Contention tools
    // =========================================================================

    // 12. contention_list
    api.registerTool(
      {
        name: "contention_list",
        label: "List Contentions",
        description:
          "List active contentions (contradictions or disagreements) in the knowledge base. " +
          "Contentions arise when a source contradicts or contends with an existing article. " +
          "Review contentions to identify knowledge that needs reconciliation.",
        parameters: Type.Object({
          article_id: Type.Optional(
            Type.String({
              description: "Optional UUID — return only contentions for this article",
            }),
          ),
          status: Type.Optional(
            stringEnum(["detected", "resolved", "dismissed"], {
              description: "Filter by status (omit to return all)",
            }),
          ),
        }),
        async execute(_id: string, params: { article_id?: string; status?: string }) {
          const result = await listContentions(cfg, {
            node_id: params.article_id,
            status: params.status,
          });
          if (!result.success) throw new Error(result.error || "contention_list failed");
          return ok(result.data);
        },
      },
      { name: "contention_list" },
    );

    // 13. contention_resolve
    api.registerTool(
      {
        name: "contention_resolve",
        label: "Resolve Contention",
        description:
          "Resolve a contention between an article and a source. " +
          "Resolution types:\n" +
          "- supersede_a: Article wins; source is noted but article unchanged.\n" +
          "- supersede_b: Source wins; article content is replaced.\n" +
          "- accept_both: Both perspectives are valid; article is annotated.\n" +
          "- dismiss: Not material; dismissed without change.",
        parameters: Type.Object({
          contention_id: Type.String({ description: "UUID of the contention to resolve" }),
          resolution: stringEnum(["supersede_a", "supersede_b", "accept_both", "dismiss"], {
            description: "Resolution type",
          }),
          rationale: Type.String({
            description: "Free-text rationale recorded on the contention",
          }),
        }),
        async execute(
          _id: string,
          params: { contention_id: string; resolution: string; rationale: string },
        ) {
          const result = await resolveContention(cfg, params.contention_id, {
            resolution: params.resolution,
            rationale: params.rationale,
          });
          if (!result.success) throw new Error(result.error || "contention_resolve failed");
          return ok(result.data);
        },
      },
      { name: "contention_resolve" },
    );

    // =========================================================================
    // Admin tools
    // =========================================================================

    // 14. admin_forget
    api.registerTool(
      {
        name: "admin_forget",
        label: "Forget Source/Article",
        description:
          "Permanently remove a source or article from the knowledge system. " +
          "For sources: deletes the source, cascades to article_sources, queues affected articles for recompilation, creates a tombstone. " +
          "For articles: deletes the article and provenance links; sources are unaffected; a tombstone is created. " +
          "This operation is IRREVERSIBLE.",
        parameters: Type.Object({
          target_type: stringEnum(["source", "article"], {
            description: "Whether to delete a source or an article",
          }),
          target_id: Type.String({ description: "UUID of the record to delete" }),
        }),
        async execute(_id: string, params: { target_type: string; target_id: string }) {
          let result;
          if (params.target_type === "source") {
            result = await deleteSource(cfg, params.target_id);
          } else {
            result = await deleteArticle(cfg, params.target_id);
          }
          if (!result.success) throw new Error(result.error || "admin_forget failed");
          return ok(result.data);
        },
      },
      { name: "admin_forget" },
    );

    // 15. admin_stats
    api.registerTool(
      {
        name: "admin_stats",
        label: "Admin Stats",
        description:
          "Return health and capacity statistics for the knowledge system. " +
          "Includes: article counts (total/active/pinned), source count, pending mutation queue depth, " +
          "tombstones (last 30 days), and bounded-memory capacity utilization.",
        parameters: Type.Object({}),
        async execute(_id: string, _params: Record<string, never>) {
          const result = await getAdminStats(cfg);
          if (!result.success) throw new Error(result.error || "admin_stats failed");
          return ok(result.data);
        },
      },
      { name: "admin_stats" },
    );

    // 16. admin_maintenance
    api.registerTool(
      {
        name: "admin_maintenance",
        label: "Admin Maintenance",
        description:
          "Trigger maintenance operations for the knowledge system. " +
          "Available operations (pass true to enable):\n" +
          "- recompute_scores: Batch-recompute usage_score for all articles.\n" +
          "- process_queue: Process pending entries in mutation_queue (recompile, split, merge_candidate, decay_check).\n" +
          "- evict_if_over_capacity: Run organic forgetting if article count exceeds the configured maximum.",
        parameters: Type.Object({
          recompute_scores: Type.Optional(
            Type.Boolean({ description: "Batch-recompute usage scores for all articles" }),
          ),
          process_queue: Type.Optional(
            Type.Boolean({ description: "Process pending entries in the mutation queue" }),
          ),
          evict_if_over_capacity: Type.Optional(
            Type.Boolean({ description: "Run organic eviction if over capacity" }),
          ),
          evict_count: Type.Optional(
            Type.Number({ description: "Maximum articles to evict per run (default 10)" }),
          ),
        }),
        async execute(_id: string, params: Record<string, unknown>) {
          const result = await runMaintenance(
            cfg,
            {
              recompute_scores: params.recompute_scores as boolean | undefined,
              process_queue: params.process_queue as boolean | undefined,
              evict_if_over_capacity: params.evict_if_over_capacity as boolean | undefined,
              evict_count: params.evict_count as number | undefined,
            },
            { timeout: 120000 },
          );
          if (!result.success) throw new Error(result.error || "admin_maintenance failed");
          return ok(result.data);
        },
      },
      { name: "admin_maintenance" },
    );

    // =========================================================================
    // Memory tools
    // =========================================================================

    // 17. memory_store
    api.registerTool(
      {
        name: "memory_store",
        label: "Store Memory",
        description:
          "Store a memory for later recall (agent-friendly wrapper). " +
          "Memories are stored as observation sources with special metadata that makes them easy for agents to search and manage. " +
          "Use this to remember important facts, learnings, decisions, or observations. " +
          "Memories can supersede previous memories and are tagged with importance and optional context tags for better retrieval.",
        parameters: Type.Object({
          content: Type.String({ description: "The memory content (required)" }),
          context: Type.Optional(
            Type.String({
              description:
                "Where this memory came from (e.g., 'session:main', 'conversation:user', 'observation:system')",
            }),
          ),
          importance: Type.Optional(
            Type.Number({
              minimum: 0.0,
              maximum: 1.0,
              description: "How important this memory is (0.0-1.0, default 0.5)",
            }),
          ),
          tags: Type.Optional(
            Type.Array(Type.String(), {
              description: "Optional categorization tags (e.g., ['infrastructure', 'decision'])",
            }),
          ),
          supersedes_id: Type.Optional(
            Type.String({ description: "UUID of a previous memory this replaces" }),
          ),
        }),
        async execute(_id: string, params: Record<string, unknown>) {
          const result = await storeMemory(cfg, {
            content: String(params.content),
            context: params.context ? String(params.context) : undefined,
            importance: typeof params.importance === "number" ? params.importance : undefined,
            tags: Array.isArray(params.tags) ? params.tags.map(String) : undefined,
            supersedes_id: params.supersedes_id ? String(params.supersedes_id) : undefined,
          });
          if (!result.success) throw new Error(result.error || "memory_store failed");
          return ok(result.data);
        },
      },
      { name: "memory_store" },
    );

    // 18. memory_recall
    api.registerTool(
      {
        name: "memory_recall",
        label: "Recall Memories",
        description:
          "Search and recall memories (agent-friendly wrapper). " +
          "Returns memories ranked by relevance, confidence, and freshness. " +
          "Results are filtered to only include observation sources marked as memories. " +
          "Optionally filter by tags or minimum confidence threshold. " +
          "Use this to retrieve relevant past knowledge before making decisions or answering questions.",
        parameters: Type.Object({
          query: Type.String({ description: "What to recall (natural language query)" }),
          limit: Type.Optional(
            Type.Number({ description: "Maximum results to return (default 5, max 50)" }),
          ),
          min_confidence: Type.Optional(
            Type.Number({
              minimum: 0.0,
              maximum: 1.0,
              description: "Optional minimum confidence threshold (0.0-1.0)",
            }),
          ),
          tags: Type.Optional(
            Type.Array(Type.String(), {
              description:
                "Optional tag filter — only return memories with at least one matching tag",
            }),
          ),
        }),
        async execute(_id: string, params: Record<string, unknown>) {
          const result = await recallMemory(cfg, {
            query: String(params.query),
            limit: typeof params.limit === "number" ? params.limit : undefined,
            min_confidence:
              typeof params.min_confidence === "number" ? params.min_confidence : undefined,
            tags: Array.isArray(params.tags) ? params.tags.map(String) : undefined,
          });
          if (!result.success) throw new Error(result.error || "memory_recall failed");
          return ok(result.data);
        },
      },
      { name: "memory_recall" },
    );

    // 19. memory_status
    api.registerTool(
      {
        name: "memory_status",
        label: "Memory Status",
        description:
          "Get statistics about the memory system. " +
          "Returns count of stored memories, articles compiled from them, last memory timestamp, and top tags. " +
          "Use this to understand the current state of the memory system.",
        parameters: Type.Object({}),
        async execute(_id: string, _params: Record<string, never>) {
          const result = await memoryStatus(cfg);
          if (!result.success) throw new Error(result.error || "memory_status failed");
          return ok(result.data);
        },
      },
      { name: "memory_status" },
    );

    // 20. memory_forget
    api.registerTool(
      {
        name: "memory_forget",
        label: "Forget Memory",
        description:
          "Mark a memory as forgotten (soft delete). " +
          "Sets the memory's metadata to include a 'forgotten' flag and optional reason. " +
          "The memory is not actually deleted from the database, but will be filtered out of future recall results. " +
          "Use this to mark outdated or incorrect memories without losing the audit trail.",
        parameters: Type.Object({
          memory_id: Type.String({ description: "UUID of the memory (source) to forget" }),
          reason: Type.Optional(
            Type.String({ description: "Optional reason why this memory is being forgotten" }),
          ),
        }),
        async execute(_id: string, params: { memory_id: string; reason?: string }) {
          const result = await forgetMemory(cfg, params.memory_id, params.reason);
          if (!result.success) throw new Error(result.error || "memory_forget failed");
          return ok(result.data);
        },
      },
      { name: "memory_forget" },
    );

    // =====================
    // FILE-BASED MEMORY TOOLS — DR fallback
    // =====================

    // Register OpenClaw's built-in memory_search and memory_get tools.
    // These operate on MEMORY.md / memory/*.md and provide a DR fallback
    // if the Covalence engine is unreachable. MUST always be available.

    try {
      const memorySearchFactory = (api as any).runtime?.tools?.createMemorySearchTool;
      const memoryGetFactory = (api as any).runtime?.tools?.createMemoryGetTool;

      if (memorySearchFactory) {
        api.registerTool(
          (ctx: any) =>
            memorySearchFactory({ config: ctx.config, agentSessionKey: ctx.sessionKey }) ?? undefined,
          { name: "memory_search", optional: true },
        );
      }

      if (memoryGetFactory) {
        api.registerTool(
          (ctx: any) =>
            memoryGetFactory({ config: ctx.config, agentSessionKey: ctx.sessionKey }) ?? undefined,
          { name: "memory_get", optional: true },
        );
      }
    } catch {
      log.warn(
        "memory-covalence: could not register file-based memory tools (runtime not available)",
      );
    }

    // =====================
    // HOOKS — Automatic lifecycle
    // =====================

    const covalenceSystemPrompt =
      "You have access to a structured knowledge base (Covalence via HTTP) with tools for: " +
      "sources (source_ingest, source_get, source_search), " +
      "articles (article_get, article_create, article_compile, article_split), " +
      "knowledge search (knowledge_search), " +
      "provenance (provenance_trace), " +
      "contentions (contention_list), " +
      "admin (admin_stats, admin_maintenance), " +
      "and memory wrappers (memory_store, memory_recall, memory_status). " +
      "Use knowledge_search BEFORE answering questions about past discussions or documented topics.";

    // Session tracking state (module-level per plugin instance)
    let currentSessionId: string | undefined;
    let currentChannel = "unknown";

    // Auto-Recall + system prompt injection
    api.on("before_agent_start", async (event: any, ctx?: any) => {
      const baseResult: { systemPrompt?: string; prependContext?: string } = {
        systemPrompt: covalenceSystemPrompt,
      };

      // Track session
      try {
        const sessionKey = ctx?.sessionKey || ctx?.sessionId;
        if (sessionKey) {
          currentSessionId = sessionKey;
          currentChannel = ctx?.messageProvider || "unknown";
        }
      } catch {
        // ignore
      }

      // Session ingestion: create/resume session in Covalence
      if (cfg.sessionIngestion && currentSessionId) {
        try {
          await createSession(cfg, {
            session_id: currentSessionId,
            channel: currentChannel,
            platform: "openclaw",
          });
        } catch {
          // Non-fatal — session may already exist
        }
      }

      if (!cfg.autoRecall || !event.prompt || event.prompt.length < 5) {
        return baseResult;
      }

      try {
        const result = await searchKnowledge(cfg, {
          query: event.prompt,
          limit: cfg.recallMaxResults,
        });

        if (!result.success || !result.data?.results) return baseResult;

        const results = result.data.results as Record<string, unknown>[];
        if (results.length === 0) return baseResult;

        const memoryContext = results
          .map((r) => {
            const title = r.title ? `**${r.title}**: ` : "";
            const content = (r.content as string)?.slice(0, 600) ?? "";
            const truncated =
              content.length < ((r.content as string)?.length ?? 0) ? "…" : "";
            return `- ${title}${content}${truncated}`;
          })
          .join("\n");

        log.info(`memory-covalence: injecting ${results.length} articles into context`);

        return {
          systemPrompt:
            covalenceSystemPrompt +
            `\n\n<relevant-knowledge>\n` +
            `The following compiled knowledge may be relevant:\n` +
            `${memoryContext}\n` +
            `</relevant-knowledge>`,
        };
      } catch (err) {
        log.warn(`memory-covalence: auto-recall failed: ${String(err)}`);
        return baseResult;
      }
    });

    // Auto-Capture: ingest observations after conversation ends
    if (cfg.autoCapture) {
      api.on("agent_end", async (event: any) => {
        if (!event.success || !event.messages || event.messages.length === 0) return;

        try {
          const texts: string[] = [];
          for (const msg of event.messages as Array<{ role: string; content: unknown }>) {
            if (msg.role !== "user" && msg.role !== "assistant") continue;
            if (typeof msg.content === "string") {
              texts.push(msg.content);
            } else if (Array.isArray(msg.content)) {
              for (const block of msg.content) {
                if (block?.type === "text" && typeof block.text === "string") {
                  texts.push(block.text);
                }
              }
            }
          }

          const capturable = texts.filter((t) => shouldCapture(t));
          if (capturable.length === 0) return;

          let captured = 0;
          for (const text of capturable.slice(0, 3)) {
            try {
              await ingestSource(cfg, {
                content: text,
                source_type: "observation",
                title: "conversation:auto-capture",
              });
              captured++;
            } catch (err) {
              log.warn(`memory-covalence: capture item failed: ${String(err)}`);
            }
          }

          if (captured > 0) {
            log.info(`memory-covalence: auto-captured ${captured} observations`);
          }
        } catch (err) {
          log.warn(`memory-covalence: auto-capture failed: ${String(err)}`);
        }
      });
    }

    // Before compaction: flush and optionally compile the session
    api.on("before_compaction", async (_event: any, ctx?: any) => {
      const sessionId = currentSessionId || ctx?.sessionKey || ctx?.sessionId;
      if (!cfg.sessionIngestion || !sessionId) return;

      try {
        // Close the session (triggers server-side flush)
        await closeSession(cfg, sessionId);
        log.info(`memory-covalence: closed session ${sessionId} (pre-compaction)`);

        // If compile-on-flush is enabled, trigger an article compile from the session source
        if (cfg.autoCompileOnFlush) {
          // The Covalence engine will handle compilation via its maintenance queue
          await runMaintenance(cfg, { process_queue: true }, { timeout: 120000 });
          log.info(`memory-covalence: triggered queue processing for session ${sessionId}`);
        }
      } catch (err) {
        log.warn(`memory-covalence: before_compaction flush failed: ${String(err)}`);
      }
    });

    // =====================
    // SERVICE — Health check
    // =====================

    api.registerService({
      id: "memory-covalence",
      async start() {
        const health = await healthCheck(cfg);
        if (health.ok) {
          log.info(
            `memory-covalence: connected to ${cfg.serverUrl} ` +
              `(v${health.version}, db: ${health.database})`,
          );
        } else {
          log.warn(
            `memory-covalence: cannot reach ${cfg.serverUrl} — ${health.error}. ` +
              `Tools will retry on use.`,
          );
        }
      },
      stop() {
        log.info("memory-covalence: stopped");
      },
    });

    // =====================
    // CLI — covalence commands
    // =====================

    api.registerCli(
      ({ program }: any) => {
        const cov = program
          .command("covalence")
          .description("Covalence knowledge graph engine (HTTP)");

        cov
          .command("status")
          .description("Check Covalence server connectivity")
          .action(async () => {
            const health = await healthCheck(cfg);
            if (health.ok) {
              console.log(
                `Connected: ${cfg.serverUrl} (v${health.version}, db: ${health.database})`,
              );
            } else {
              console.error(`Not connected: ${health.error}`);
              process.exitCode = 1;
            }
          });

        cov
          .command("search")
          .description("Search knowledge")
          .argument("<query>", "Search query")
          .option("--limit <n>", "Max results", "10")
          .action(async (query: string, opts: { limit: string }) => {
            const result = await searchKnowledge(cfg, {
              query,
              limit: parseInt(opts.limit, 10),
            });

            if (!result.success) {
              console.error("Search failed:", result.error);
              process.exitCode = 1;
              return;
            }

            const results = (result.data?.results ?? []) as Record<string, unknown>[];
            if (results.length === 0) {
              console.log("No results found.");
              return;
            }

            for (const r of results) {
              const title = r.title ? `[${r.title}] ` : "";
              const content = (r.content as string)?.slice(0, 100) ?? "";
              console.log(`${title}${content}...`);
            }
          });

        cov
          .command("ingest")
          .description("Ingest a source")
          .argument("<content>", "Source content")
          .option("--type <t>", "Source type", "observation")
          .option("--title <title>", "Title")
          .action(async (content: string, opts: { type: string; title?: string }) => {
            const result = await ingestSource(cfg, {
              content,
              source_type: opts.type,
              title: opts.title,
            });
            if (!result.success) {
              console.error("Ingest failed:", result.error);
              process.exitCode = 1;
              return;
            }
            console.log("Source ingested:", result.data);
          });

        cov
          .command("stats")
          .description("Show knowledge system statistics")
          .action(async () => {
            const result = await getAdminStats(cfg);
            if (!result.success) {
              console.error("Stats failed:", result.error);
              process.exitCode = 1;
              return;
            }
            console.log(JSON.stringify(result.data, null, 2));
          });
      },
      { commands: ["covalence"] },
    );

    // =====================
    // INFERENCE ENDPOINTS
    // =====================

    if (cfg.inferenceEnabled) {
      registerInferenceEndpoints(api, {
        inferenceModel: cfg.inferenceModel,
        chatModel: cfg.chatModel,
        embeddingModel: cfg.embeddingModel,
      });
    }
  },
};

export default covalencePlugin;
