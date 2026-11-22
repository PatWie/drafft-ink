import { describe, expect, it } from "vitest";

import { originAllowed } from "../src/origin";
import { QuotaGate } from "../src/quota";
import { parseRoute } from "../src/routes";
import { legalCloseCode } from "../src/sockets";

describe("routes", () => {
  it("reads the paths of the relay", () => {
    expect(parseRoute("/api/status")).toEqual({ kind: "status" });
    expect(parseRoute("/r/abc/ws")).toEqual({ kind: "socket", room: "abc" });
    expect(parseRoute("/r/abc/f")).toEqual({ kind: "images", room: "abc" });
    expect(parseRoute("/r/abc/f/xyz")).toEqual({ kind: "image", room: "abc", name: "xyz" });
  });

  it("knows nothing else", () => {
    for (const path of ["/", "/r", "/r/abc", "/r/abc/", "/r/abc/ws/", "/r/abc/f/x/y", "/r//ws", "/api", "/api/status/x"]) {
      expect(parseRoute(path), path).toEqual({ kind: "unknown" });
    }
  });
});

describe("origin", () => {
  const url = new URL("https://drafft.ink/r/x/ws");
  it("lets a native client through", () => {
    expect(originAllowed(null, url, "https://drafft.ink")).toBe(true);
  });
  it("checks a browser against the list", () => {
    expect(originAllowed("https://drafft.ink", url, "https://drafft.ink, https://other.example")).toBe(true);
    expect(originAllowed("https://other.example/", url, "https://drafft.ink https://other.example")).toBe(true);
    expect(originAllowed("https://evil.example", url, "https://drafft.ink")).toBe(false);
    expect(originAllowed("null", url, "https://drafft.ink")).toBe(false);
  });
  it("allows only its own origin when the list is empty", () => {
    expect(originAllowed("https://drafft.ink", url, "")).toBe(true);
    expect(originAllowed("https://evil.example", url, "  ")).toBe(false);
    expect(originAllowed("http://drafft.ink", url, "")).toBe(false);
  });
});

describe("quota gate", () => {
  it("answers until the reset and not after", () => {
    const gate = new QuotaGate();
    expect(gate.retryAfterSeconds(1_000)).toBeNull();
    gate.trip(61_000);
    expect(gate.retryAfterSeconds(1_000)).toBe(60);
    expect(gate.retryAfterSeconds(60_999)).toBe(1);
    expect(gate.retryAfterSeconds(61_000)).toBeNull();
  });
  it("never moves the reset back", () => {
    const gate = new QuotaGate();
    gate.trip(10_000);
    gate.trip(5_000);
    expect(gate.retryAfterSeconds(0)).toBe(10);
  });
});

describe("close codes", () => {
  it("never allows the codes that must not be sent", () => {
    for (const code of [1004, 1005, 1006, 1015, 0, 999, 2999, 5000]) expect(legalCloseCode(code), String(code)).toBe(false);
    for (const code of [1000, 1001, 1003, 1007, 1011, 1013, 1014, 3000, 4999]) expect(legalCloseCode(code), String(code)).toBe(true);
  });
});
