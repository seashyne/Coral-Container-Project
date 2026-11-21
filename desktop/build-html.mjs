#!/usr/bin/env node
/**
 * Assembles the Coral Desktop frontend shell.
 *
 * Source of truth: `src/index.template.html` + `src/partials/*.html`
 * Generated output:  `src/index.html` (git-ignored, rebuilt by every
 *                    `npm run dev` / `npm run build` / `npm run tauri` script)
 */

import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { assemble } from "../tools/html-assembler.mjs";

const here = dirname(fileURLToPath(import.meta.url));

const { out, bytes } = assemble({
  template: resolve(here, "src/index.template.html"),
  out: resolve(here, "src/index.html"),
  banner:
    "GENERATED FILE — edit src/index.template.html and src/partials/*.html instead.",
  root: resolve(here, "src"),
});

console.log(`coral-desktop: assembled ${out} (${bytes} bytes)`);
