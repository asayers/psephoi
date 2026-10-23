import init from "client"; // Comes from /assets/ via the importmap
await init({ module_or_path: import.meta.resolve("client.wasm") });
