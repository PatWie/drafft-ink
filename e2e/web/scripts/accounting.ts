/**
 * The accounting check of the browser suite (`npm run accounting`, run by `cargo xtask e2e`).
 *
 * It reads the specs and the harness with the TypeScript compiler API and fails when:
 *  - a method of `window.__drafftinkTest` (harness/harness.page.ts) has no `Reason:` line in its doc comment;
 *  - the methods of the harness, of the `DrafftinkTest` interface and of the dispatcher differ;
 *  - a harness method is declared and no spec calls it (delete it);
 *  - a spec or a support file calls a harness method that is not declared;
 *  - a command family of the registry has no `family <name>` test in specs/families;
 *  - a story of e2e/stories.txt has no test where it says it runs (browser title, native `story_<id>` function);
 *  - the native suite has a `story_*` test that stories.txt does not list.
 * `commands.d.ts` freshness is `tsx scripts/gen-command-types.ts --check`; types are `tsc --noEmit`.
 * The checks have their own tests: `npm run accounting -- --self-test` feeds them broken input.
 */
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const here = path.dirname(fileURLToPath(import.meta.url));
const WEB = path.resolve(here, "..");
const REPO_E2E = path.resolve(WEB, "..");

/** Source text by file name. */
type Sources = Map<string, string>;

/** Everything the checks read. */
export interface Inputs {
  harness: string;
  harnessTypes: string;
  specs: Sources;
  support: Sources;
  families: string[];
  stories: string;
  nativeStories: string;
}

function parse(name: string, text: string): ts.SourceFile {
  return ts.createSourceFile(name, text, ts.ScriptTarget.ES2022, true);
}

function visit(node: ts.Node, f: (n: ts.Node) => void): void {
  f(node);
  ts.forEachChild(node, (c) => visit(c, f));
}

/** Method names of the object literal assigned to `const harness: DrafftinkTest`, with their doc comments. */
export function declaredMethods(harnessSource: string): Map<string, string> {
  const file = parse("harness.page.ts", harnessSource);
  const found = new Map<string, string>();
  visit(file, (n) => {
    if (ts.isVariableDeclaration(n) && ts.isIdentifier(n.name) && n.name.text === "harness" && n.initializer && ts.isObjectLiteralExpression(n.initializer)) {
      for (const member of n.initializer.properties) {
        if (!member.name || !ts.isIdentifier(member.name)) continue;
        const docs = ts.getJSDocCommentsAndTags(member).map((d) => d.getText(file)).join("\n");
        found.set(member.name.text, docs);
      }
    }
  });
  return found;
}

/** Method names of `interface DrafftinkTest`. */
export function interfaceMethods(typesSource: string): Set<string> {
  const file = parse("harness.types.ts", typesSource);
  const names = new Set<string>();
  visit(file, (n) => {
    if (ts.isInterfaceDeclaration(n) && n.name.text === "DrafftinkTest") {
      for (const m of n.members) if (m.name && ts.isIdentifier(m.name)) names.add(m.name.text);
    }
  });
  return names;
}

/** Harness method names called as `probe(page, "name", ...)` or read as `__drafftinkTest.name`. */
export function calledMethods(source: string): Set<string> {
  const file = parse("spec.ts", source);
  const names = new Set<string>();
  visit(file, (n) => {
    if (ts.isCallExpression(n) && ts.isIdentifier(n.expression) && n.expression.text === "probe") {
      const second = n.arguments[1];
      if (second && ts.isStringLiteralLike(second)) names.add(second.text);
    }
    if (ts.isPropertyAccessExpression(n) && n.expression.getText(file).endsWith("__drafftinkTest")) names.add(n.name.text);
  });
  return names;
}

/** Titles of `test("...")` calls (also `test.describe`-free: the title string only). */
export function testTitles(source: string): string[] {
  const file = parse("spec.ts", source);
  const titles: string[] = [];
  visit(file, (n) => {
    if (ts.isCallExpression(n)) {
      const callee = n.expression;
      const isTest = ts.isIdentifier(callee) && callee.text === "test";
      const first = n.arguments[0];
      if (isTest && first && ts.isStringLiteralLike(first)) titles.push(first.text);
    }
  });
  return titles;
}

/** All problems found in `inputs`; an empty list is a pass. */
export function check(inputs: Inputs): string[] {
  const problems: string[] = [];
  const declared = declaredMethods(inputs.harness);
  const iface = interfaceMethods(inputs.harnessTypes);

  for (const [name, doc] of declared) {
    const m = /Reason:\s*(\S[^]*)/.exec(doc);
    if (!m || m[1] === undefined || m[1].trim().length < 40) problems.push(`harness method \`${name}\` has no real \`Reason:\` line (what can a command not do here?)`);
    if (!iface.has(name)) problems.push(`harness method \`${name}\` is not in the DrafftinkTest interface`);
  }
  for (const name of iface) if (!declared.has(name)) problems.push(`DrafftinkTest declares \`${name}\` but harness.page.ts does not implement it`);

  const usedBySpecs = new Set<string>();
  for (const [, text] of inputs.specs) for (const n of calledMethods(text)) usedBySpecs.add(n);
  const usedBySupport = new Set<string>();
  for (const [, text] of inputs.support) for (const n of calledMethods(text)) usedBySupport.add(n);
  for (const name of declared.keys()) if (!usedBySpecs.has(name)) problems.push(`harness method \`${name}\` is declared and no spec calls it: delete it`);
  for (const name of new Set([...usedBySpecs, ...usedBySupport])) if (!declared.has(name)) problems.push(`\`${name}\` is called but not declared in harness.page.ts`);

  const titles = [...inputs.specs.values()].flatMap(testTitles);
  const duplicates = titles.filter((t, i) => titles.indexOf(t) !== i);
  for (const t of new Set(duplicates)) problems.push(`two tests have the title "${t}"`);
  const familyTitles = titles.filter((t) => t.startsWith("family "));
  for (const family of inputs.families) {
    const covered = familyTitles.some((t) => {
      const head = (t.slice("family ".length).split(":")[0] ?? "").split(/,| and /).map((s) => s.trim());
      return head.includes(family);
    });
    if (!covered) problems.push(`command family \`${family}\` has no \`family ${family}: ...\` test in specs/families`);
  }

  const listed = new Map<string, { where: string; words: string }>();
  for (const line of inputs.stories.split("\n")) {
    if (line.trim() === "" || line.startsWith("#")) continue;
    const [id, where, words] = line.split("|").map((s) => s.trim());
    if (!id || !where || !words) {
      problems.push(`stories.txt: cannot read the line "${line}"`);
      continue;
    }
    listed.set(id, { where, words });
  }
  for (const [id, { where, words }] of listed) {
    if (!titles.some((t) => t.includes(words))) problems.push(`story \`${id}\`: no browser test title contains "${words}"`);
    if (where === "both") {
      const fn = `story_${id.replaceAll("-", "_")}`;
      if (!new RegExp(`fn\\s+${fn}\\b`).test(inputs.nativeStories)) problems.push(`story \`${id}\` runs in both suites, and the native suite has no \`fn ${fn}\``);
    } else if (where !== "web") {
      problems.push(`story \`${id}\`: "where" is \`${where}\`, expected both or web`);
    }
  }
  for (const m of inputs.nativeStories.matchAll(/fn\s+story_([a-z0-9_]+)\b/g)) {
    const id = (m[1] ?? "").replaceAll("_", "-");
    const entry = listed.get(id);
    if (!entry || entry.where !== "both") problems.push(`native test \`story_${m[1]}\` is not a "both" story in stories.txt`);
  }
  return problems;
}

function readTree(dir: string, suffix: string): Sources {
  const out: Sources = new Map();
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) {
      for (const [k, v] of readTree(full, suffix)) out.set(k, v);
    } else if (entry.endsWith(suffix)) {
      out.set(path.relative(WEB, full), readFileSync(full, "utf8"));
    }
  }
  return out;
}

function realInputs(): Inputs {
  const registry = JSON.parse(readFileSync(path.join(WEB, "generated/registry.json"), "utf8")) as { families: string[] };
  const nativePath = path.join(REPO_E2E, "native/tests/stories.rs");
  return {
    harness: readFileSync(path.join(WEB, "harness/harness.page.ts"), "utf8"),
    harnessTypes: readFileSync(path.join(WEB, "harness/harness.types.ts"), "utf8"),
    specs: readTree(path.join(WEB, "specs"), ".spec.ts"),
    support: readTree(path.join(WEB, "support"), ".ts"),
    families: registry.families,
    stories: readFileSync(path.join(REPO_E2E, "stories.txt"), "utf8"),
    nativeStories: existsSync(nativePath) ? readFileSync(nativePath, "utf8") : "",
  };
}

/** The checks, run on broken inputs: each must report the problem it is for. */
function selfTest(): string[] {
  const failures: string[] = [];
  const good = realInputs();
  const base = check(good);
  if (base.length > 0) failures.push(`the real inputs should pass the self-test base: ${base.join("; ")}`);
  const expectProblem = (what: string, inputs: Inputs, fragment: string): void => {
    const found = check(inputs);
    if (!found.some((p) => p.includes(fragment))) failures.push(`${what}: expected a problem containing "${fragment}", got ${JSON.stringify(found)}`);
  };
  expectProblem("an unused harness method", { ...good, specs: new Map([...good.specs].map(([k, v]) => [k, v.replaceAll('"pasteText"', '"canvasInfo"')])) }, "`pasteText` is declared and no spec calls it");
  expectProblem("an undeclared call", { ...good, specs: new Map([...good.specs, ["specs/x.spec.ts", 'probe(page, "teleport");']]) }, "`teleport` is called but not declared");
  expectProblem("a missing reason", { ...good, harness: good.harness.replace(/Reason:/g, "Because:") }, "no real `Reason:` line");
  expectProblem("an interface without the method", { ...good, harnessTypes: good.harnessTypes.replace("pasteText(text: string): void;", "") }, "`pasteText` is not in the DrafftinkTest interface");
  expectProblem("an uncovered family", { ...good, families: [...good.families, "teleport"] }, "`teleport` has no `family teleport");
  expectProblem("a story without a test", { ...good, stories: good.stories + "ghost | web | a test nobody wrote\n" }, 'no browser test title contains "a test nobody wrote"');
  expectProblem("a native story that is not listed", { ...good, nativeStories: good.nativeStories + "\nfn story_ghost() {}\n" }, "`story_ghost` is not a \"both\" story");
  expectProblem("a both story without the native test", { ...good, nativeStories: "" }, "the native suite has no `fn story_arrow_attached`");
  return failures;
}

function main(): void {
  if (process.argv.includes("--self-test")) {
    const failures = selfTest();
    for (const f of failures) console.error(`self-test FAILED: ${f}`);
    if (failures.length > 0) process.exit(1);
    console.log("accounting self-test: every check reports the problem it is for");
    return;
  }
  const inputs = realInputs();
  const problems = check(inputs);
  for (const p of problems) console.error(`accounting: ${p}`);
  if (problems.length > 0) process.exit(1);
  const declared = declaredMethods(inputs.harness);
  const tests = [...inputs.specs.values()].flatMap(testTitles).length;
  console.log(`accounting ok: ${declared.size} harness methods, all called; ${inputs.families.length} families covered; ${tests} tests`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
