/**
 * Proof that the specs are really typed against the command schemas. Each `@ts-expect-error` below is a call that
 * must NOT compile: if the generated types ever turned loose (`any`), the directive would be unused and `tsc --noEmit`
 * would fail on it. This file is never run.
 */
import type { App } from "./app.ts";

export async function neverRun(app: App): Promise<void> {
  // @ts-expect-error a command that is not in the registry
  await app.exec("element.teleport", {});
  // @ts-expect-error a required argument is missing (`kind`)
  await app.exec("element.create", { bounds: { x: 0, y: 0, w: 1, h: 1 } });
  // @ts-expect-error an argument the command does not have
  await app.exec("element.move", { ids: ["a"], dz: 3 });
  // @ts-expect-error the wrong type for an argument
  await app.exec("element.move", { ids: ["a"], dx: "far" });
  // @ts-expect-error a value outside the enum of the argument
  await app.exec("tool.set", { tool: "banana" });
  // @ts-expect-error arguments are required for this command
  await app.exec("element.resize");
  // @ts-expect-error a batch step is typed like a call
  await app.batch([{ id: "element.move", args: { ids: ["a"], dx: "far" } }]);
  // Calls that must compile:
  await app.exec("doc.new");
  await app.exec("element.move", { ids: ["a"], dx: 1 });
}
