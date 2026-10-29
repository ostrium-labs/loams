// wrangler's Text rule (wrangler.toml): these files import as strings.
declare module "*.sql" {
  const text: string;
  export default text;
}
declare module "*.toml" {
  const text: string;
  export default text;
}
declare module "*.conf" {
  const text: string;
  export default text;
}
