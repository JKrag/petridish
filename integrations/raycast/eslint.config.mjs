// ESLint flat config.
//
// Replaces `.eslintrc.json`. ESLint 9 stopped defaulting to the eslintrc format
// and ESLint 10 removed it, so keeping the old file meant pinning eslint at 8
// forever — which is what Dependabot kept pointing out.
//
// `@raycast/eslint-config` v2 exports a flat config array directly (v1 was
// eslintrc-only), so the two upgrades have to land together: eslint 10 with
// config v1 fails, and config v2 with eslint 8 fails.
//
// `.mjs` rather than `.js` because package.json has no `"type": "module"`, so a
// plain `.js` here would be CommonJS — and the config's own rules forbid
// `require()`, meaning the config file would fail its own lint.
import raycast from "@raycast/eslint-config";

export default [
  // Nothing in these is ours to lint, and `node_modules` in particular turns a
  // seconds-long run into a minutes-long one.
  { ignores: ["node_modules/**", "dist/**", "raycast-env.d.ts"] },
  ...raycast,
];
