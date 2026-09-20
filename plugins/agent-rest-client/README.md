# Agent Rest Client

An HTTP client inside Agentty: pick a method, type a URL, add headers and a body, send it and read
the status, time, size and response — without leaving the window.

It is a Rust program compiled to WebAssembly, so it runs the same on macOS, Windows and Linux and
cannot reach your files or your credentials. Its one permission is `net.request`: every request
goes through Agentty's `net/fetch`, which bounds the method, the headers, the sizes, the redirects
and the time, adds nothing of yours to the request, and writes each call to the plugin's log with
the URL redacted.

## Build and install

```sh
./build.sh
```

Then in Agentty: **Plugins → Install from Folder…** and pick this folder.

## Using it

| | |
|---|---|
| Method | `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS` |
| URL | `http` or `https`; press Enter to send |
| Headers | `Accept: application/json; Authorization: Bearer …` — separated by `;` |
| Body | sent with `POST`, `PUT` and `PATCH` |
| Recent | the last eight requests; click one to load it back |

The response shows the status, how long it took, how many bytes came back and the content type,
with the body below it.
