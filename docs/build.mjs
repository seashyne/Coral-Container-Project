#!/usr/bin/env node
/**
 * Assembles the Coral documentation site entry page.
 *
 * Source of truth: `index.template.html` + `partials/*.html`
 * Generated output:  `index.html` (git-ignored, rebuilt by the GitHub Pages
 *                    workflow before the site artifact is uploaded)
 *
 * Local preview:  node docs/build.mjs
 */

import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { assemble } from "../tools/html-assembler.mjs";

const here = dirname(fileURLToPath(import.meta.url));

const { out, bytes } = assemble({
  template: resolve(here, "index.template.html"),
  out: resolve(here, "index.html"),
  banner:
    "GENERATED FILE — edit index.template.html and partials/*.html instead.",
  root: here,
});

console.log(`coral-docs: assembled ${out} (${bytes} bytes)`);
