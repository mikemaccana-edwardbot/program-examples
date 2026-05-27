// Generate Rust clients for TukTuk's `tuktuk` and `cron` programs.
//
// Codama's Rust renderer assumes one program per crate and emits absolute
// paths like `crate::CRON_ID` and `crate::generated::types::*`. We want both
// clients living side-by-side inside our stop-loss-vault program crate as
// `crate::tuktuk_client` and `crate::cron_client`. So after rendering we do
// a deterministic path rewrite of the generated files.
//
// Usage:
//   cd anchor
//   npm install --no-save codama @codama/renderers-rust @codama/nodes-from-anchor
//   node scripts/codama-gen-tuktuk.mjs
//
// Source IDLs live in `anchor/idls/`. They came from
// https://github.com/quiknode-labs/tuktuk → `tuktuk-program/idls/`. Refresh
// them by re-cloning and copying when TukTuk publishes a new release.
import { createFromRoot } from "codama";
import { rootNodeFromAnchor } from "@codama/nodes-from-anchor";
import { renderVisitor } from "@codama/renderers-rust";
import {
  readFileSync,
  writeFileSync,
  readdirSync,
  statSync,
  cpSync,
  rmSync,
  mkdirSync,
  existsSync,
} from "node:fs";
import { resolve, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const anchorRoot = resolve(here, "..");
const idlDir = resolve(anchorRoot, "idls");
const programSrc = resolve(
  anchorRoot,
  "programs/stop-loss-vault/src",
);

const programs = [
  {
    idl: "tuktuk.json",
    moduleName: "tuktuk_client",
    idConst: "TUKTUK_ID",
  },
  {
    idl: "cron.json",
    moduleName: "cron_client",
    idConst: "CRON_ID",
  },
];

const walk = (dir, files = []) => {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walk(full, files);
    else if (full.endsWith(".rs")) files.push(full);
  }
  return files;
};

for (const { idl, moduleName, idConst } of programs) {
  const stagingDir = join(tmpdir(), `codama-stage-${moduleName}`);
  console.log(
    `\n=== Generating ${idl} -> ${programSrc}/${moduleName} (via ${stagingDir}) ===`,
  );
  const raw = JSON.parse(readFileSync(join(idlDir, idl), "utf8"));
  const root = rootNodeFromAnchor(raw);
  const codama = createFromRoot(root);
  codama.accept(
    renderVisitor(stagingDir, {
      anchorTraits: true,
      deleteFolderBeforeRendering: true,
      formatCode: false,
      syncCargoToml: false,
    }),
  );

  // Path rewrite: the generator emits `crate::CRON_ID` / `crate::TUKTUK_ID`
  // and `crate::generated::types::Foo`. Rewrite to live under
  // `crate::<moduleName>` in stop-loss-vault.
  const generatedDir = join(stagingDir, "src", "generated");
  for (const file of walk(generatedDir)) {
    let src = readFileSync(file, "utf8");
    src = src.replace(/\bcrate::generated::/g, `crate::${moduleName}::`);
    src = src.replace(
      new RegExp(`\\bcrate::${idConst}\\b`, "g"),
      `crate::${moduleName}::${idConst}`,
    );
    src = src.replace(
      /\bcrate::shared::/g,
      `crate::${moduleName}::shared::`,
    );
    writeFileSync(file, src);
  }

  // The generator emits an `errors/` module that depends on `num_derive`
  // and `thiserror` — deps we don't want in an onchain program. Suppress
  // the module reference in the freshly generated mod.rs before copying.
  const stagedMod = join(generatedDir, "mod.rs");
  const modText = readFileSync(stagedMod, "utf8").replace(
    /^\s*pub mod errors;\s*$/m,
    "        // errors module suppressed by codama-gen-tuktuk.mjs (num_derive/thiserror not wanted onchain)\n        // pub mod errors;",
  );
  writeFileSync(stagedMod, modText);

  // Replace the in-tree module with the freshly generated one.
  const target = join(programSrc, moduleName);
  if (existsSync(target)) rmSync(target, { recursive: true, force: true });
  mkdirSync(target, { recursive: true });
  cpSync(generatedDir, target, { recursive: true });
  console.log(`✅ ${moduleName} written`);
}
