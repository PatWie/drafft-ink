/**
 * Writes `generated/commands.d.ts` (and `generated/registry.json`, the command ids and families): one TypeScript type per command argument list and output, made from the
 * JSON Schemas of the command registry. The specs call `app.exec("element.create", {...})`, so a wrong
 * argument name or type is a compile error (`tsc --noEmit`), not a failure at run time.
 *
 * Input: `generated/commands.json`, the output of `engine.commands {include_schemas: true}`:
 *   cargo run -p drafftink-e2e --example command_schemas > e2e/web/generated/commands.json
 * `cargo xtask e2e` does this first.
 *
 * usage: tsx scripts/gen-command-types.ts [--check]
 *   --check  fail when the committed `generated/commands.d.ts` differs from what the registry gives now
 */
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

type Schema = boolean | { [key: string]: unknown };
type Obj = { [key: string]: unknown };

interface CommandInfo {
  id: string;
  family: string;
  summary?: string;
  args_schema?: Obj | null;
  output_schema?: Obj | null;
}

const here = path.dirname(fileURLToPath(import.meta.url));
const INPUT = path.join(here, "../generated/commands.json");
const OUTPUT = path.join(here, "../generated/commands.d.ts");
const REGISTRY = path.join(here, "../generated/registry.json");

const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

function isObj(v: unknown): v is Obj {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function jsdoc(text: unknown, indent: string): string {
  if (typeof text !== "string" || text.trim() === "") return "";
  const lines = text.replaceAll("*/", "* /").trim().split("\n");
  if (lines.length === 1) return `${indent}/** ${lines[0]} */\n`;
  return `${indent}/**\n${lines.map((l) => `${indent} * ${l}`.trimEnd()).join("\n")}\n${indent} */\n`;
}

/** Converts a JSON Schema (draft 2020-12 as `schemars` writes it) to a TypeScript type expression. */
function toType(schema: Schema, out: boolean, depth: number): string {
  const prefix = out ? "O_" : "A_";
  if (schema === true) return "unknown";
  if (schema === false) return "never";
  const s = schema;
  const ref = s["$ref"];
  if (typeof ref === "string") {
    const m = /^#\/\$defs\/(.+)$/.exec(ref);
    if (!m || m[1] === undefined) throw new Error(`unsupported $ref ${ref}`);
    return `${prefix}${m[1].replace(/\W/g, "_")}`;
  }
  if ("const" in s) return JSON.stringify(s["const"]);
  if (Array.isArray(s["enum"])) return s["enum"].map((v) => JSON.stringify(v)).join(" | ");
  for (const key of ["oneOf", "anyOf"] as const) {
    const branches = s[key];
    if (Array.isArray(branches)) {
      const parts = [...new Set(branches.map((b) => toType(b as Schema, out, depth)))];
      return parts.length === 0 ? "never" : parts.join(" | ");
    }
  }
  if (Array.isArray(s["allOf"])) {
    return s["allOf"].map((b) => `(${toType(b as Schema, out, depth)})`).join(" & ");
  }
  const type = s["type"];
  if (Array.isArray(type)) {
    return [...new Set(type.map((t) => toType({ ...s, type: t }, out, depth)))].join(" | ");
  }
  switch (type) {
    case "string":
      return "string";
    case "integer":
    case "number":
      return "number";
    case "boolean":
      return "boolean";
    case "null":
      return "null";
    case "array":
      return arrayType(s, out, depth);
    case "object":
      return objectType(s, out, depth);
    default:
      if (isObj(s["properties"])) return objectType(s, out, depth);
      return "unknown";
  }
}

function arrayType(s: Obj, out: boolean, depth: number): string {
  if (Array.isArray(s["prefixItems"])) {
    return `[${s["prefixItems"].map((i) => toType(i as Schema, out, depth)).join(", ")}]`;
  }
  const items = s["items"] === undefined ? true : (s["items"] as Schema);
  const item = toType(items, out, depth);
  const min = s["minItems"];
  const max = s["maxItems"];
  if (typeof min === "number" && min === max && min >= 1 && min <= 4) {
    return `[${Array<string>(min).fill(item).join(", ")}]`;
  }
  return /[|&]/.test(item) ? `(${item})[]` : `${item}[]`;
}

function objectType(s: Obj, out: boolean, depth: number): string {
  const props = isObj(s["properties"]) ? s["properties"] : {};
  const required = new Set(Array.isArray(s["required"]) ? (s["required"] as string[]) : []);
  const extra = s["additionalProperties"];
  const pad = "  ".repeat(depth + 1);
  const lines: string[] = [];
  for (const [name, sub] of Object.entries(props)) {
    const key = IDENT.test(name) ? name : JSON.stringify(name);
    const doc = isObj(sub) ? jsdoc(sub["description"], pad) : "";
    lines.push(`${doc}${pad}${key}${required.has(name) ? "" : "?"}: ${toType(sub as Schema, out, depth + 1)};`);
  }
  if (isObj(extra)) {
    lines.push(`${pad}[key: string]: ${toType(extra, out, depth + 1)};`);
  } else if (extra !== false && (out || Object.keys(props).length === 0)) {
    // Outputs may carry keys the schema does not list; an empty object accepts any JSON object.
    lines.push(`${pad}[key: string]: unknown;`);
  }
  if (lines.length === 0) return "Record<string, never>";
  return `{\n${lines.join("\n")}\n${"  ".repeat(depth)}}`;
}

function pascal(id: string): string {
  return id
    .split(/[._]/)
    .map((p) => p.charAt(0).toUpperCase() + p.slice(1))
    .join("");
}

/** The `$defs` of a schema as aliases, then the root as `name`. */
function emitSchema(name: string, schema: Obj, out: boolean): string {
  const lines: string[] = [];
  const defs = isObj(schema["$defs"]) ? schema["$defs"] : {};
  for (const [defName, def] of Object.entries(defs)) {
    const doc = isObj(def) ? jsdoc(def["description"], "  ") : "";
    lines.push(`${doc}  type ${out ? "O_" : "A_"}${defName.replace(/\W/g, "_")} = ${toType(def as Schema, out, 1)};`);
  }
  lines.push(`${jsdoc(schema["description"], "  ")}  export type ${name} = ${toType({ ...schema, $defs: undefined }, out, 1)};`);
  return lines.join("\n");
}

function generate(commands: CommandInfo[]): string {
  const parts: string[] = [
    "// Generated by e2e/web/scripts/gen-command-types.ts from `engine.commands {include_schemas: true}`.",
    "// Do not edit. Regenerate with `cargo xtask e2e --types` (or the first step of `cargo xtask e2e`).",
    "",
  ];
  const table: string[] = [];
  const optional: string[] = [];
  for (const c of [...commands].sort((a, b) => a.id.localeCompare(b.id))) {
    const ns = pascal(c.id);
    const args = c.args_schema ?? { type: "object" };
    const output = c.output_schema ?? { type: "object" };
    const doc = jsdoc(c.summary, "").trimEnd();
    if (doc) parts.push(doc);
    parts.push(`export namespace ${ns} {`, emitSchema("Args", args, false), emitSchema("Output", output, true), "}", "");
    table.push(`  ${JSON.stringify(c.id)}: { args: ${ns}.Args; output: ${ns}.Output };`);
    const required = Array.isArray(args["required"]) ? (args["required"] as unknown[]) : [];
    const isObject = args["type"] === "object" || isObj(args["properties"]);
    if (required.length === 0 && isObject) optional.push(JSON.stringify(c.id));
  }
  parts.push("/** Every command: its arguments and its output. */", "export interface CommandTable {", ...table, "}", "");
  parts.push("export type CommandId = keyof CommandTable;", "");
  parts.push("/** Commands whose arguments may all be left out. */");
  parts.push(`export type OptionalArgsId = ${optional.length ? optional.join(" | ") : "never"};`, "");
  parts.push(`/** The registry size when the types were made; a spec can compare it with \`commands()\`. */`);
  parts.push(`export declare const COMMAND_COUNT: ${commands.length};`, "");
  return parts.join("\n");
}

function main(): void {
  if (!existsSync(INPUT)) {
    throw new Error(`${INPUT} not found. Run: cargo run -p drafftink-e2e --example command_schemas > e2e/web/generated/commands.json`);
  }
  const raw: unknown = JSON.parse(readFileSync(INPUT, "utf8"));
  if (!isObj(raw) || !Array.isArray(raw["commands"])) throw new Error("commands.json is not an engine.commands result");
  const commands = raw["commands"] as CommandInfo[];
  const text = generate(commands);
  const registry = JSON.stringify({ count: commands.length, families: [...new Set(commands.map((c) => c.family))].sort(), ids: commands.map((c) => c.id).sort() }, null, 1) + "\n";
  if (process.argv.includes("--check")) {
    const have = existsSync(OUTPUT) ? readFileSync(OUTPUT, "utf8") : "";
    const haveRegistry = existsSync(REGISTRY) ? readFileSync(REGISTRY, "utf8") : "";
    if (have !== text || haveRegistry !== registry) {
      console.error("generated/commands.d.ts is out of date with the command registry. Run `cargo xtask e2e --types` and commit it.");
      process.exit(1);
    }
    console.log(`commands.d.ts is current (${(raw["commands"] as unknown[]).length} commands)`);
    return;
  }
  writeFileSync(OUTPUT, text);
  writeFileSync(REGISTRY, registry);
  console.log(`wrote ${path.relative(process.cwd(), OUTPUT)}: ${(raw["commands"] as unknown[]).length} commands`);
}

main();
