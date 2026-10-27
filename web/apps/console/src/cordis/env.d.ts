/// <reference types="vite-plus/client" />

declare module '*.yml?raw' {
  const text: string;
  export default text;
}
