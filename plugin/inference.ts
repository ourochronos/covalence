/**
 * OpenAI-compatible inference proxy endpoints for the Covalence plugin.
 *
 * Exposes:
 *   POST /covalence/v1/chat/completions  — chat completions (OpenAI format)
 *   POST /covalence/v1/embeddings        — embeddings (OpenAI format)
 *
 * The Covalence engine sets OPENAI_BASE_URL=http://localhost:18789/covalence/v1
 * to route its LLM calls through these endpoints, which in turn use OpenClaw's
 * configured model providers.
 */

import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import type { IncomingMessage, ServerResponse } from "http";

// =========================================================================
// Types
// =========================================================================

interface ChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

interface ChatCompletionRequest {
  model?: string;
  messages: ChatMessage[];
  max_tokens?: number;
  temperature?: number;
  stream?: boolean;
}

interface EmbeddingRequest {
  input: string | string[];
  model?: string;
}

// =========================================================================
// Registration
// =========================================================================

export function registerInferenceEndpoints(
  api: OpenClawPluginApi,
  config: {
    inferenceModel?: string;
    chatModel: string;
    embeddingModel: string;
  },
): void {
  const log = api.logger;

  // -------------------------------------------------------------------------
  // POST /covalence/v1/chat/completions
  // -------------------------------------------------------------------------
  api.registerHttpRoute({
    path: "/covalence/v1/chat/completions",
    handler: async (req: IncomingMessage, res: ServerResponse) => {
      if (req.method !== "POST") {
        res.writeHead(405, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "Method not allowed" }));
        return;
      }

      let body: ChatCompletionRequest;
      try {
        body = await readJsonBody<ChatCompletionRequest>(req);
      } catch (err) {
        res.writeHead(400, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: `Invalid JSON body: ${String(err)}` }));
        return;
      }

      if (!body.messages || body.messages.length === 0) {
        res.writeHead(400, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "Missing or empty 'messages' field" }));
        return;
      }

      // Resolve model: request model > inferenceModel config > chatModel default
      const modelId = body.model || config.inferenceModel || config.chatModel;

      try {
        const { content, inputTokens, outputTokens } = await callChatModel(
          api,
          modelId,
          body.messages,
          body.max_tokens,
          body.temperature,
        );

        const responseId = `chatcmpl-cov-${Date.now()}`;
        const timestamp = Math.floor(Date.now() / 1000);

        const openAiResponse = {
          id: responseId,
          object: "chat.completion",
          created: timestamp,
          model: modelId,
          choices: [
            {
              index: 0,
              message: {
                role: "assistant",
                content,
              },
              finish_reason: "stop",
            },
          ],
          usage: {
            prompt_tokens: inputTokens,
            completion_tokens: outputTokens,
            total_tokens: inputTokens + outputTokens,
          },
        };

        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify(openAiResponse));
      } catch (err) {
        log.warn(`covalence-inference: chat/completions failed: ${String(err)}`);
        res.writeHead(502, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: String(err) }));
      }
    },
  });

  // -------------------------------------------------------------------------
  // POST /covalence/v1/embeddings
  // -------------------------------------------------------------------------
  api.registerHttpRoute({
    path: "/covalence/v1/embeddings",
    handler: async (req: IncomingMessage, res: ServerResponse) => {
      if (req.method !== "POST") {
        res.writeHead(405, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "Method not allowed" }));
        return;
      }

      let body: EmbeddingRequest;
      try {
        body = await readJsonBody<EmbeddingRequest>(req);
      } catch (err) {
        res.writeHead(400, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: `Invalid JSON body: ${String(err)}` }));
        return;
      }

      if (!body.input) {
        res.writeHead(400, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "Missing 'input' field" }));
        return;
      }

      const modelId = body.model || config.embeddingModel;
      const inputs = Array.isArray(body.input) ? body.input : [body.input];

      try {
        const embeddings = await callEmbeddingModel(api, modelId, inputs);

        const data = embeddings.map((embedding, index) => ({
          object: "embedding",
          index,
          embedding,
        }));

        // Rough token estimate: ~4 chars per token
        const totalChars = inputs.reduce((sum, s) => sum + s.length, 0);
        const estimatedTokens = Math.ceil(totalChars / 4);

        const openAiResponse = {
          object: "list",
          model: modelId,
          data,
          usage: {
            prompt_tokens: estimatedTokens,
            total_tokens: estimatedTokens,
          },
        };

        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify(openAiResponse));
      } catch (err) {
        log.warn(`covalence-inference: embeddings failed: ${String(err)}`);
        res.writeHead(502, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: String(err) }));
      }
    },
  });

  log.info(
    "covalence-inference: registered POST /covalence/v1/chat/completions and /covalence/v1/embeddings",
  );
}

// =========================================================================
// Model dispatch — chat completions
// =========================================================================

async function callChatModel(
  api: OpenClawPluginApi,
  modelSpec: string,
  messages: ChatMessage[],
  maxTokens?: number,
  temperature?: number,
): Promise<{ content: string; inputTokens: number; outputTokens: number }> {
  // Parse "provider/model" or just "model"
  let targetProvider: string | undefined;
  let targetModelId: string | undefined;

  if (modelSpec.includes("/")) {
    const [p, m] = modelSpec.split("/", 2);
    targetProvider = p;
    targetModelId = m;
  } else {
    targetModelId = modelSpec;
  }

  // --- Anthropic auth-profile path ---
  if (
    targetProvider === "anthropic" ||
    (!targetProvider && targetModelId?.startsWith("claude"))
  ) {
    const credential = await resolveAuthProfileCredential(api, "anthropic");
    if (credential) {
      const model = targetModelId || "claude-sonnet-4-20250514";
      api.logger.info(
        `covalence-inference: Anthropic auth-profile (${credential.type}), model=${model}`,
      );
      return callAnthropicChat(credential, model, messages, maxTokens, temperature);
    }
    if (targetProvider === "anthropic") {
      throw new Error("Anthropic provider requested but no auth-profile credential found");
    }
  }

  // --- OpenClaw models.providers path ---
  const modelsConfig = (api.config as any)?.models;
  if (!modelsConfig?.providers) {
    throw new Error(
      "No model providers configured in OpenClaw and no matching auth-profile found",
    );
  }

  for (const [providerName, providerConfig] of Object.entries(modelsConfig.providers)) {
    if (targetProvider && providerName !== targetProvider) continue;

    for (const model of (providerConfig as any).models || []) {
      if (targetModelId && model.id !== targetModelId) continue;

      const baseUrl: string = ((providerConfig as any).baseUrl || "").replace(/\/$/, "");
      const apiKey: string | undefined = (providerConfig as any).apiKey;

      if (!baseUrl) throw new Error(`Provider '${providerName}' has no baseUrl`);

      const headers: Record<string, string> = { "Content-Type": "application/json" };
      if (apiKey) headers["Authorization"] = `Bearer ${apiKey}`;
      if ((providerConfig as any).headers) Object.assign(headers, (providerConfig as any).headers);

      const requestBody: Record<string, unknown> = {
        model: model.id,
        messages,
      };
      if (maxTokens !== undefined) requestBody.max_tokens = maxTokens;
      if (temperature !== undefined) requestBody.temperature = temperature;

      const response = await fetch(`${baseUrl}/chat/completions`, {
        method: "POST",
        headers,
        body: JSON.stringify(requestBody),
      });

      if (!response.ok) {
        const errorText = await response.text().catch(() => "");
        throw new Error(`Model API returned ${response.status}: ${errorText.slice(0, 500)}`);
      }

      const data = (await response.json()) as any;
      const content = data?.choices?.[0]?.message?.content;
      if (typeof content !== "string") {
        throw new Error(
          `Unexpected response from model API: ${JSON.stringify(data).slice(0, 500)}`,
        );
      }

      const inputTokens = data?.usage?.prompt_tokens ?? estimateTokens(messages.map((m) => m.content).join(" "));
      const outputTokens = data?.usage?.completion_tokens ?? estimateTokens(content);

      return { content, inputTokens, outputTokens };
    }
  }

  throw new Error(
    targetModelId
      ? `Model '${modelSpec}' not found in OpenClaw providers or auth-profiles`
      : "No models available in OpenClaw providers",
  );
}

// =========================================================================
// Model dispatch — embeddings
// =========================================================================

async function callEmbeddingModel(
  api: OpenClawPluginApi,
  modelSpec: string,
  inputs: string[],
): Promise<number[][]> {
  let targetProvider: string | undefined;
  let targetModelId: string | undefined;

  if (modelSpec.includes("/")) {
    const [p, m] = modelSpec.split("/", 2);
    targetProvider = p;
    targetModelId = m;
  } else {
    targetModelId = modelSpec;
  }

  // Look for a provider that supports embeddings
  const modelsConfig = (api.config as any)?.models;
  if (!modelsConfig?.providers) {
    throw new Error("No model providers configured in OpenClaw for embeddings");
  }

  for (const [providerName, providerConfig] of Object.entries(modelsConfig.providers)) {
    if (targetProvider && providerName !== targetProvider) continue;

    const pCfg = providerConfig as any;
    const baseUrl: string = (pCfg.baseUrl || "").replace(/\/$/, "");
    const apiKey: string | undefined = pCfg.apiKey;

    if (!baseUrl) continue;

    // Find a matching model (embedding models may not be listed under "models")
    // We try the requested model directly
    const headers: Record<string, string> = { "Content-Type": "application/json" };
    if (apiKey) headers["Authorization"] = `Bearer ${apiKey}`;
    if (pCfg.headers) Object.assign(headers, pCfg.headers);

    const requestBody: Record<string, unknown> = {
      model: targetModelId || modelSpec,
      input: inputs.length === 1 ? inputs[0] : inputs,
    };

    const response = await fetch(`${baseUrl}/embeddings`, {
      method: "POST",
      headers,
      body: JSON.stringify(requestBody),
    });

    if (!response.ok) {
      const errorText = await response.text().catch(() => "");
      // If this provider doesn't support embeddings, try next
      if (response.status === 404 || response.status === 400) {
        api.logger.warn(
          `covalence-inference: provider '${providerName}' doesn't support embeddings (${response.status}), trying next`,
        );
        continue;
      }
      throw new Error(`Embeddings API returned ${response.status}: ${errorText.slice(0, 500)}`);
    }

    const data = (await response.json()) as any;
    const embeddingData = data?.data;

    if (!Array.isArray(embeddingData)) {
      throw new Error(
        `Unexpected embeddings response: ${JSON.stringify(data).slice(0, 500)}`,
      );
    }

    // Sort by index and extract embedding vectors
    const sorted = [...embeddingData].sort((a: any, b: any) => (a.index ?? 0) - (b.index ?? 0));
    return sorted.map((item: any) => item.embedding as number[]);
  }

  throw new Error(
    `No provider found that supports embeddings model '${modelSpec}'`,
  );
}

// =========================================================================
// Anthropic direct call (for chat only — no embedding support)
// =========================================================================

async function callAnthropicChat(
  credential: { token: string; type: "token" | "api_key" },
  model: string,
  messages: ChatMessage[],
  maxTokens?: number,
  temperature?: number,
): Promise<{ content: string; inputTokens: number; outputTokens: number }> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    "anthropic-version": "2023-06-01",
  };

  if (credential.type === "token") {
    headers["Authorization"] = `Bearer ${credential.token}`;
    headers["anthropic-beta"] = "oauth-2025-04-20";
  } else {
    headers["x-api-key"] = credential.token;
  }

  // Separate system message from conversation
  const systemMsg = messages.find((m) => m.role === "system");
  const conversationMsgs = messages.filter((m) => m.role !== "system");

  const requestBody: Record<string, unknown> = {
    model,
    max_tokens: maxTokens ?? 4096,
    messages: conversationMsgs,
  };
  if (systemMsg) requestBody.system = systemMsg.content;
  if (temperature !== undefined) requestBody.temperature = temperature;

  const response = await fetch("https://api.anthropic.com/v1/messages", {
    method: "POST",
    headers,
    body: JSON.stringify(requestBody),
  });

  if (!response.ok) {
    const errorText = await response.text().catch(() => "");
    throw new Error(`Anthropic API returned ${response.status}: ${errorText.slice(0, 500)}`);
  }

  const data = (await response.json()) as any;
  const content = data?.content?.[0]?.text;

  if (typeof content !== "string") {
    throw new Error(
      `Unexpected Anthropic response format: ${JSON.stringify(data).slice(0, 500)}`,
    );
  }

  const inputTokens = data?.usage?.input_tokens ?? estimateTokens(messages.map((m) => m.content).join(" "));
  const outputTokens = data?.usage?.output_tokens ?? estimateTokens(content);

  return { content, inputTokens, outputTokens };
}

// =========================================================================
// Helpers
// =========================================================================

function estimateTokens(text: string): number {
  return Math.ceil(text.length / 4);
}

async function readJsonBody<T>(req: IncomingMessage): Promise<T> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf-8")) as T;
}

async function resolveAuthProfileCredential(
  api: OpenClawPluginApi,
  provider: string,
): Promise<{ token: string; type: "token" | "api_key" } | undefined> {
  const { readFileSync, readdirSync, existsSync } = await import("fs");
  const { join } = await import("path");

  const homeDir = process.env.HOME || process.env.USERPROFILE || "";
  const stateDir = process.env.OPENCLAW_STATE_DIR || join(homeDir, ".openclaw");
  const agentsDir = join(stateDir, "agents");

  if (!existsSync(agentsDir)) return undefined;

  try {
    const agents = readdirSync(agentsDir);
    for (const agentId of agents) {
      const profilePath = join(agentsDir, agentId, "agent", "auth-profiles.json");
      if (!existsSync(profilePath)) continue;

      try {
        const data = JSON.parse(readFileSync(profilePath, "utf-8"));
        const profiles = data.profiles || data;

        for (const [, profile] of Object.entries(profiles)) {
          const p = profile as any;
          if (p.provider !== provider) continue;
          if (p.type === "api_key" && p.key) return { token: p.key, type: "api_key" };
          if (p.type === "token" && p.token) return { token: p.token, type: "token" };
        }
      } catch {
        // Skip malformed files
      }
    }
  } catch {
    // agents dir not readable
  }

  return undefined;
}
