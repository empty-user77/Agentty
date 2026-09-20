# Agent Rest Client

An HTTP client inside Agentty, in the shape you expect: a request with a method, URL, headers, a
body and authorization; environments whose values fill in `{{placeholders}}`; a collection of saved
requests; a proxy; and the response with its status, time, size, headers and pretty-printed JSON.

It is a Rust program compiled to WebAssembly, so it has no files, no processes and no network of
its own. Requests go out through Agentty's `net/fetch` (the `net.request` permission), which bounds
the method, the headers, the sizes, the redirects and the time, adds nothing of yours to the
request, and writes each call to the plugin's log with the URL redacted. What you save is kept in
this plugin's own folder under `plugin-data`, in a file only you can read.

## Build and install

```sh
./build.sh
```

Then in Agentty: **Plugins → Install from Folder…** and pick this folder. Its icon appears in the
activity bar on the left.

## The four views

**Request** — method, URL (Enter sends it), Save with a name, headers as rows, authorization
(bearer token, basic, or an API key header), a body for `POST` / `PUT` / `PATCH`, and the response.
Paste a JSON body and its line breaks are kept: the field shows a summary, the body itself is shown
below it, and **Format JSON** tidies it.

**Environments** — name an environment, give it values, and use `{{name}}` anywhere in a request:
the URL, a header, the body, a token, even the proxy. What has no value is left as it is and
Agentty says which placeholder was empty, so a half-filled request is never sent silently.

**Collection** — saved requests. Pick one to load it back, or delete it.

**Settings** — a proxy (`http://127.0.0.1:8888`, with `user:password@` if yours asks for it) and a
timeout in seconds (60 at most).

## What is not here

Postman's tests and scripting: this plugin runs no JavaScript. Cookies are not kept between
requests, and a request body is pasted rather than typed over many lines — Agentty's panel fields
are single-line.
