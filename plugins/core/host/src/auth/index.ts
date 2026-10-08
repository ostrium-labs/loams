/**
 * Authentication for the plugin platform.
 *
 * The security model is in `docs/security.md`. The Authentik setup, and the five
 * silent-failure traps this module exists to report legibly rather than swallow,
 * are in `docs/auth-setup.md`.
 */

export * from "./errors.js";
export * from "./config.js";
export * from "./jwks.js";
export * from "./oidc.js";
export * from "./session.js";
export * from "./scopes.js";
export * from "./tokens.js";
export * from "./backchannel.js";
export * from "./service.js";
export * from "./routes.js";
