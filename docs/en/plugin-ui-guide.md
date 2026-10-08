---
title: Designing a panel
description: Which element to use for what, a handful of rules that make a panel look like part of Agentty, and whole screens to start from.
---

A plugin never draws: it sends a tree of elements and Agentty draws them, in the app's own colors, type and spacing. That is why every plugin can look as good as Agentty itself — and why the way to get there is choosing the right element, not styling one.

> [!TIP]
> Install **UI Gallery** from **Plugins** (it comes with Agentty). It shows every element live, with the code that draws it under each one, and the recipes on this page as screens you can click through.

## Pick the element for the job

| You want to show | Use | Not |
|---|---|---|
| Things the user picks from (notes, tasks, sessions) | `list` — icon, title, subtitle, detail, buttons on hover | a column of buttons |
| Records with the same fields (runs, files, requests) | `table` — headings, aligned columns, a click selects a row | a list with the fields joined into a subtitle |
| The details of one thing | `keyValue`, inside a `card` | lines of `text` with colons |
| A number that matters | `stat`, several in a `grid` | a `title` text |
| What belongs together | `card` (raised box) or `section` (a quiet heading) | `divider`s between everything |
| Several views of the same plugin | `tabs`, sending only the picked tab's content | a `choice` at the top |
| How far something has got | `progress`; `spinner` when you cannot say | a percentage in text |
| A hint, a warning, what went wrong | `callout` with a `tone` | `text` with style `error` for a whole paragraph |
| Settings | `input`, `select`, `checkbox`, `toggle` in `section`s | free text for a fixed set of options |
| Code, commands, output | `code` with a `language` | `text` with style `code` for more than one line |
| Steps an automation goes through | `flow` | a numbered list |

## Rules that make it look right

1. **One primary button per screen.** It is the thing the user came to do. Everything else is `secondary`, or `ghost` for Cancel and the like; `danger` only for what cannot be undone.
2. **Group, then space.** Put related elements in a `card` or a `section`, and leave the spacing to Agentty — `gap` is the only spacing there is, and `medium` is right almost everywhere.
3. **Tone means something.** `success`, `warning` and `error` say how things are; `info` points at something. A tone used for decoration stops meaning anything.
4. **Every list has an empty state and every call has an error state.** Give `list` and `table` an `empty` text that says what to do next, and show failures in a `callout` with a way to try again.
5. **Show that work is happening.** A `spinner` or a `progress` the moment something starts — a panel that does not change after a click looks broken.
6. **Short words, sentence case.** "Open in a tab", not "OPEN IN NEW TAB". Put the detail in a subtitle or `detail`, not the title.
7. **The panel is narrow.** It starts at 360 px: two columns in a `grid` is plenty, a `table` wants three or four columns at most, and a `row` shares its width between text and fields, with buttons at their own size.
8. **Speak the user's language.** `context.language` is `en`, `ko`, `ja` or `zh`; keep the strings in a table.

## Recipes

Whole screens, ready to copy. The same code is in UI Gallery under **Recipes**.

### List and detail

```js
ui.column([
  ui.row([ui.input('search', { placeholder: 'Search tasks' }), ui.button('new', 'New', { variant: 'primary', icon: 'plus' })]),
  ui.list('tasks', tasks.map((t) => ({ id: t.id, title: t.title, subtitle: t.agent, icon: t.done ? 'circle-check' : 'loader-circle', tone: t.done ? 'success' : 'info' })),
    { empty: 'No tasks yet. Press New to start one.' }),
  picked && ui.card([
    ui.keyValue([
      { label: 'Status', value: picked.done ? 'Done' : 'Working', tone: picked.done ? 'success' : 'info' },
      { label: 'Branch', value: picked.branch, mono: true },
    ]),
    ui.row([ui.button('open', 'Open in a tab', { icon: 'external-link' }), ui.button('delete', 'Delete', { variant: 'ghost' })], { gap: 'small' }),
  ], { title: picked.title, icon: 'file-text' }),
]);
```

### Settings form

```js
ui.column([
  ui.section('Connection', [
    ui.input('url', { placeholder: 'https://api.example.com', value: settings.url }),
    ui.select('env', [{ value: 'dev', label: 'Development' }, { value: 'prod', label: 'Production' }], settings.env),
  ]),
  ui.section('Behavior', [
    ui.toggle('watch', 'Watch for changes', settings.watch),
    ui.checkbox('confirm', 'Ask before sending prompts', settings.confirm, { description: 'Shows the prompt to the user first.' }),
  ]),
  ui.row([ui.button('save', 'Save', { variant: 'primary' }), ui.button('reset', 'Reset', { variant: 'ghost' })], { gap: 'small' }),
]);
```

### Dashboard

```js
ui.column([
  ui.grid([
    ui.stat('Open PRs', String(prs.length), { detail: '+2 today', tone: 'info', icon: 'git-pull-request' }),
    ui.stat('CI', ci.ok ? 'passing' : 'failing', { tone: ci.ok ? 'success' : 'error', icon: ci.ok ? 'circle-check' : 'circle-x' }),
  ]),
  ui.section('Recent runs', [
    ui.table('runs', [{ label: 'Workflow', grow: 3 }, { label: 'Result', grow: 2 }, { label: 'Time', align: 'end' }],
      runs.map((r) => ({ id: r.id, cells: [r.name, r.result, r.duration], tone: r.ok ? 'success' : 'error' }))),
  ]),
]);
```

### A job that is running

```js
ui.card([
  ui.progress(done / total, { label: `Step ${done + 1} of ${total} · ${step}` }),
  ui.code(output, { language: 'sh' }),
  ui.row([ui.button('stop', 'Stop', { variant: 'danger', icon: 'square' })]),
], { title: 'Release check', subtitle: `started ${since}`, icon: 'rocket', tone: 'info' });
```

### Empty and error

```js
ui.column([
  ui.list('notes', [], { empty: 'No notes yet. Create one to get started.' }),
  ui.callout('Check the token in Settings and try again.', { title: 'Could not sign in', tone: 'error' }),
  ui.row([ui.button('retry', 'Try again', { variant: 'primary', icon: 'refresh-cw' })]),
]);
```

## The elements

The [Node.js SDK](/docs/plugin-sdk#the-panel) lists every builder with its options and events, and the [protocol](/docs/plugin-protocol#ui-tree) the JSON each one sends. In short:

| Element | Events |
|---|---|
| `column`, `row`, `grid`, `section`, `card`, `divider` | |
| `tabs` | `change` with the tab's id |
| `text`, `badge`, `keyValue`, `stat`, `code`, `callout`, `progress`, `spinner` | |
| `list` | `select` with `item`; row buttons `action` with `item` and `action` |
| `table` | `select` with `item` (the row's id) |
| `flow` | `select` with `item` (the step's id) |
| `button` | `click` |
| `input` | `change` after a pause, `submit` on Enter |
| `select`, `choice` | `change` with the option's value |
| `checkbox`, `toggle` | `change` with the new boolean |
| `popover` | `close` |

`card`, `grid`, `tabs`, `table`, `keyValue`, `stat`, `progress`, `callout`, `select`, `checkbox` and `code` are API version 4: say `"apiVersion": 4` in the manifest when you use them, and an older Agentty asks the user to update instead of showing a panel it cannot draw. `host/info` lists what the running Agentty draws in `uiFeatures`, for a plugin that wants to work on both.

## Next

- [Node.js SDK](/docs/plugin-sdk) · [Rust and WebAssembly](/docs/plugin-rust)
- [Plugin protocol](/docs/plugin-protocol) — the UI tree as JSON, and its limits
