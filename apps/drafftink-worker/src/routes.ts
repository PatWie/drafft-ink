/** What a path asks for. The room id and the picture name are checked later, by the Rust core's parser. */
export type Route =
  | { kind: "status" }
  | { kind: "legacy_socket" }
  | { kind: "socket"; room: string }
  | { kind: "images"; room: string }
  | { kind: "image"; room: string; name: string }
  | { kind: "unknown" };

const SOCKET = /^\/r\/([^/]+)\/ws$/;
const IMAGES = /^\/r\/([^/]+)\/f$/;
const IMAGE = /^\/r\/([^/]+)\/f\/([^/]+)$/;

/** Reads a request path (the part before `?`). */
export function parseRoute(pathname: string): Route {
  if (pathname === "/api/status") return { kind: "status" };
  if (pathname === "/ws") return { kind: "legacy_socket" };
  const socket = SOCKET.exec(pathname);
  if (socket?.[1] !== undefined) return { kind: "socket", room: socket[1] };
  const images = IMAGES.exec(pathname);
  if (images?.[1] !== undefined) return { kind: "images", room: images[1] };
  const image = IMAGE.exec(pathname);
  if (image?.[1] !== undefined && image[2] !== undefined) return { kind: "image", room: image[1], name: image[2] };
  return { kind: "unknown" };
}
