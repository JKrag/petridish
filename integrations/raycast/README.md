# petri (Raycast extension)

Reads `~/.petridish/projects.json` (written by `swab scan`) and renders it as
a Raycast list, grouped by `active` / `in_flight` / `stale` / `cold`. Read-only
— never writes that file. See `ARCHITECTURE.md` §7 in the repo root for a summary, or
`docs/archive/IMPLEMENTATION_PLAN.md` §9 for the full original design notes and known gaps.

## Setup

```sh
cd raycast
npm install
```

## Develop / run inside Raycast

```sh
npm run dev   # ray develop — requires the Raycast app installed
```

## Check without Raycast running

```sh
npx tsc --noEmit   # typecheck
npm test           # pure-logic unit tests (Node's built-in test runner)
npm run lint        # ray lint (needs ~/.config/raycast to be writable)
npm run build       # ray build (same)
```

`swab scan` must have run at least once (real `~/.petridish/projects.json`
present) to see actual data in the list; otherwise it shows the same
"no state file" message the CLI and `petri` both print.

## Status: not published to the Raycast Store

Two things block `ray publish`, both real rather than merely unfinished:

1. **Licence conflict.** This repo is GPL-3.0-or-later; the Raycast Store requires
   MIT. `package.json` currently declares MIT, which is the inherited default and
   not a decision anyone made — resolving it properly is a prerequisite, not a
   formality.
2. **Placeholder icon.** `assets/icon.png` is not real artwork.

`npm run lint` (`ray lint`) also fails offline: it calls Raycast's API to validate
the `author` field, which 404s for a handle not registered with Raycast. CI
therefore runs `eslint` and `prettier` directly, which are the parts of that
wrapper that actually check this code.

Until then this is a clone-and-`npm run dev` extension, which works fine for
personal use.
