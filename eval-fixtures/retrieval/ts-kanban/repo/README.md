# tackboard

A small self-hosted kanban board. Plain TypeScript in the browser (no UI
framework), Express + SQLite on the server.

## Features

- Columns and cards with drag-and-drop reordering (fractional rank keys)
- Undo / redo of local edits
- Optimistic writes with an offline queue that survives reloads
- Live updates over WebSocket with per-field conflict handling
- Member roles: owner, admin, editor, commenter, viewer
- Search box with `label:`, `@person`, `due:` and `is:archived` filters
- Due-date reminders via browser notifications, shown in each member's time zone
- Markdown card descriptions (sanitized)
- Keyboard shortcuts (`?` shows the list)
- English and French UI strings

## Layout

```
src/shared   types, rank keys, roles/capabilities, dates, UI strings
src/client   store, reducer, network layers, input handling
src/server   express app, auth, sqlite access, websocket hub, routes
test/        vitest unit tests
```

## Running

```bash
npm install
npm run server   # API on :8080, websocket on /live
npm run dev      # frontend on :5173
npm test
```
