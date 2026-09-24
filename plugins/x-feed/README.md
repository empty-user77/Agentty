# X Feed

Collect posts from X accounts, keep them refined with their images and videos, pick the best and
turn them into drafts in your own style, ready to post.

```
collect → refine → select → convert → (ready to post) → post
```

Agentty keeps the in-app browser and your sign-in; this plugin never sees a password or a cookie.
It reads the profiles you follow at a person's pace (a few posts per run, a pause between each).

## Where everything is

Under the plugin's data folder (`~/.agentty/plugin-data/x-feed/`):

```
x/
  accounts/<account>.json          every post id collected for the account (and its stage), its runs
  <day>/<account>/                 what was collected that day (the local date)
    runs.json                      each run: new, already collected, why it stopped, gap
    posts/<post id>/
      raw.json                     what the page showed + the post's embed data
      post.json                    the refined post (below)
      post.md                      the same, to read
      images/  videos/             its media, full size
drafts/<draft id>/
  draft.json  draft.md             the converted post, its checks and history
  media/images/  media/videos/     the media that go with it
  source.md  draft.txt             for AI styles: what the agent read and wrote
styles.json                        your styles
```

A post is collected once. A run stops at the first posts it already has ("caught up"), at its
limit, or where the timeline ends; a run that hits its limit before meeting known posts marks the
account with a **gap**, and **Continue** reads on past the known posts to fill it.

`post.json`: `id`, `url`, `kind` (`post` · `quote` · `reply` · `repost`), `pinned`, `author`,
`postedAt`, `text` (whole, links expanded), `quote`, `hashtags`, `mentions`, `links`,
`metrics` (`replies`, `reposts`, `likes`, `views`, as numbers, with when they were read),
`media` (`kind`, `path`, `posterPath`, `error`), `status` and `statusAt`.

## Stages

| Post | |
|---|---|
| `refined` | collected and cleaned up |
| `selected` / `skipped` | picked for converting, or set aside |
| `drafted` | a draft was made from it (`draftId`) |
| `uploaded` | posted (the last step, coming) |

| Draft | |
|---|---|
| `draft` | made; edit it |
| `ready` | checked against what X accepts (length as X counts it, 4 images or 1 video) — ready to post |
| `uploaded` · `discarded` | posted, or thrown away (its posts go back to `selected`) |

## Styles

A **template** fills in `{text} {short} {name} {handle} {account} {date} {url} {likes} {hashtags}`,
one draft per post or one digest for all of them (`header`, `item` per post, `footer` with
`{urls}`). An **AI rewrite** opens a Claude Code session in the draft's folder with the style's
instructions; the draft fills in once the agent has written `draft.txt`.

## Tests

```sh
node --test plugins/x-feed/test/*.test.mjs
```
