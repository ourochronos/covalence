/**
 * Local type shim for openclaw/plugin-sdk.
 * The real types are provided at runtime by the OpenClaw host process.
 * This stub satisfies the TypeScript compiler during build/typecheck without
 * imposing false-positive type errors from an incomplete API surface.
 */
declare module "openclaw/plugin-sdk" {
  export interface Logger {
    debug(...args: unknown[]): void;
    info(...args: unknown[]): void;
    warn(...args: unknown[]): void;
    error(...args: unknown[]): void;
  }

  // Intentionally permissive — the real SDK ships with full types at runtime.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  export type OpenClawPluginApi = Record<string, any>;
}
