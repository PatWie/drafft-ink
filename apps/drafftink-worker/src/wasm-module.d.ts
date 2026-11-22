// wrangler turns a `.wasm` import into a compiled WebAssembly.Module.
declare module "*.wasm" {
  const module: WebAssembly.Module;
  export default module;
}
