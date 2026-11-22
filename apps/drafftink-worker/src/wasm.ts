// The Rust core, compiled by `npm run build:wasm` into pkg/. wrangler bundles the .wasm file as a module, and
// `initSync` instantiates it once per isolate, before any request is served.
import wasmModule from "../pkg/drafftink_worker_bg.wasm";
import { initSync } from "../pkg/drafftink_worker.js";

initSync({ module: wasmModule });

export {
  access_check_claims,
  access_parse,
  access_refusal,
  Effects,
  EffectKind,
  ImageCall,
  ImageReply,
  MemberCheck,
  RoomCore,
  checkMember,
  checkSettings,
  dailyLimitFrame,
  isImageName,
  isRoomId,
  nextUtcMidnightMs,
  r2Key,
  r2Prefix,
  roomIdOfToken,
  sealedImageLimit,
} from "../pkg/drafftink_worker.js";
export type { BlobHost, SqlHost } from "../pkg/drafftink_worker.js";
