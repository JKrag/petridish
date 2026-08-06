# petri (Raycast extension)

Reads `~/.petridish/projects.json` (written by `swab scan`) and renders it as
a Raycast list, grouped by `active` / `in_flight` / `stale` / `cold`. Read-only
— never writes that file. See `IMPLEMENTATION_PLAN.md` §9 in the repo root for
the design notes and known gaps.

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
