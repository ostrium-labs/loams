// Replaced at build time by electron-vite `define` (electron.vite.config.ts).
// Empty means the updater is disabled. Separate from the CLI release key.
export const UPDATE_PUBKEY_HEX: string = process.env.LOAMS_UPDATE_PUBKEY ?? "";
/** Base URL of the generic feed (`latest*.yml` + `.sig` + artifacts). Empty disables updates. */
export const UPDATE_FEED: string = process.env.LOAMS_UPDATE_FEED ?? "";
