/**
 * Configuration for the Covalence memory plugin.
 */

export type CovalenceConfig = {
  serverUrl: string;
  authToken?: string;
  autoRecall: boolean;
  autoCapture: boolean;
  recallMaxResults: number;
  recallMinScore: number;
  captureDomains: string[];
  sessionIngestion: boolean;
  staleSessionMinutes: number;
  autoCompileOnFlush: boolean;
  includeSystemMessages: boolean;
  inferenceEnabled: boolean;
  inferenceToken?: string;
  inferenceModel?: string;
  embeddingModel: string;
  chatModel: string;
};

export const covalenceConfigSchema = {
  parse(value: unknown): CovalenceConfig {
    const raw = (value ?? {}) as Record<string, unknown>;

    const serverUrl = resolveEnvVar(String(raw.serverUrl ?? "http://localhost:8430"));
    const authToken = raw.authToken ? resolveEnvVar(String(raw.authToken)) : undefined;
    const inferenceToken = raw.inferenceToken ? resolveEnvVar(String(raw.inferenceToken)) : undefined;

    return {
      serverUrl: serverUrl.replace(/\/+$/, ""),
      authToken,
      inferenceToken,
      autoRecall: raw.autoRecall !== false,
      autoCapture: raw.autoCapture === true,
      recallMaxResults: typeof raw.recallMaxResults === "number" ? raw.recallMaxResults : 5,
      recallMinScore: typeof raw.recallMinScore === "number" ? raw.recallMinScore : 0.3,
      captureDomains: Array.isArray(raw.captureDomains)
        ? raw.captureDomains.map(String)
        : ["conversations"],
      sessionIngestion: raw.sessionIngestion !== false,
      staleSessionMinutes: typeof raw.staleSessionMinutes === "number" ? raw.staleSessionMinutes : 30,
      autoCompileOnFlush: raw.autoCompileOnFlush !== false,
      includeSystemMessages: raw.includeSystemMessages !== false,
      inferenceEnabled: typeof raw.inferenceEnabled === "boolean" ? raw.inferenceEnabled : true,
      inferenceModel: typeof raw.inferenceModel === "string" ? raw.inferenceModel : undefined,
      embeddingModel: typeof raw.embeddingModel === "string" ? raw.embeddingModel : "text-embedding-3-small",
      chatModel: typeof raw.chatModel === "string" ? raw.chatModel : "gpt-4.1-mini",
    };
  },
};

function resolveEnvVar(value: string): string {
  return value.replace(/\$\{(\w+)\}/g, (_match, name) => {
    const envValue = process.env[name];
    if (!envValue) throw new Error(`Environment variable ${name} is not set`);
    return envValue;
  });
}
