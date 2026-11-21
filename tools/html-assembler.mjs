/**
 * Shared HTML assembler for the Coral project.
 *
 * Plain HTML has no include mechanism, so large documents are kept as small
 * `partials/*.html` files and assembled into the final page by a tiny build
 * step. The source of truth is the template plus its partials; the assembled
 * page is a generated artifact and is git-ignored.
 *
 * Template syntax: a line containing only an include directive
 *
 *     <!-- @include partials/head.html -->
 *
 * is replaced by the verbatim contents of that file (including its trailing
 * newline). Includes may nest; the include depth is capped to catch cycles.
 *
 * The assembler is intentionally dependency-free and deterministic: the same
 * inputs always produce byte-identical output, which keeps the generated page
 * diffable against the previous hand-written version.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const INCLUDE_LINE = /^[ \t]*<!--[ \t]*@include[ \t]+(.+?)[ \t]*-->[ \t]*\r?\n/gm;
const MAX_DEPTH = 16;

/**
 * Expands every `<!-- @include path -->` directive in `file`.
 *
 * @param {string} file absolute path of the file to expand
 * @param {number} depth current recursion depth
 * @returns {string} expanded contents
 */
function expand(file, depth) {
  if (depth > MAX_DEPTH) {
    throw new Error(`include depth exceeded while expanding ${file}`);
  }
  const source = readFileSync(file, "utf8");
  return source.replace(INCLUDE_LINE, (_match, target) => {
    const resolved = resolve(dirname(file), target);
    return expand(resolved, depth + 1);
  });
}

/**
 * Assembles a template into its output file.
 *
 * @param {{template: string, out: string, banner?: string, root?: string}} options
 * @returns {{out: string, bytes: number}} summary of the write
 */
export function assemble({ template, out, banner = "", root }) {
  const templatePath = resolve(template);
  const outputPath = resolve(out);
  const body = expand(templatePath, 0);

  if (root) {
    const rootPath = resolve(root);
    const rel = outputPath.startsWith(rootPath) ? outputPath : outputPath;
    if (!rel.startsWith(rootPath)) {
      throw new Error(`refusing to write outside of ${rootPath}: ${outputPath}`);
    }
  }

  // The banner marks the file as generated and is appended after the closing
  // tag so it can never influence parsing (a comment before <!doctype> is
  // tolerated but pointless, a comment inside <head> would need a partial).
  const payload = banner ? `${body}<!-- ${banner} -->\n` : body;
  writeFileSync(outputPath, payload, "utf8");
  return { out: outputPath, bytes: Buffer.byteLength(payload, "utf8") };}
