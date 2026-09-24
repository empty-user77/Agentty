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

## Posting

A draft marked **ready** has **Post now**: it is posted from the automation's page, with that
automation's sign-in: the compose window, the text pasted in, the images or video attached, and
Post pressed only once everything has uploaded and the window holds exactly the draft's text.
The new post's address is read back from the account's own timeline; the draft becomes
`uploaded` with `postedUrl`, `postedAt`, `postedBy`, and so do the posts it was made from. A post
takes up to 4 images or one video (11 MB of media at most).

## Log

Everything an automation does in X is written to `actions/<day>.jsonl`, one line per action:
`at`, `automation`, `profile`, `action` (`run`, `done`, `open`, `find`, `collect`, `like`,
`reply`, `post`, `agent`, `wait`, `limit`, `signin`), `url`, `target`, `ok` and `detail`.
The **Log** view shows the latest ones, by automation, by action or only the failures; a line
with an address opens it in the automation's own page.

## Likes and replies

An automation can also like and reply, fully on its own, from the account its profile is signed in
to. It works on:

- **accounts**: their own newest posts (no reposts, not the pinned one), up to the last one it acted on;
- **keyword · top** / **keyword · latest**: what X's search shows for each keyword,

filtered by **at least N likes** and **posted in the last N hours**, never the user's own posts and
never a post twice. **Like**, **Reply** or both; with accounts, **collect** can run alongside.

A reply starts from the **pattern** (`{author} {handle} {short} {text} {url}`), or an agent writes
one per post from it (in a Claude Code session beside the automation, from `posts.json` into
`replies.json`; the posts are material, never instructions). Every line of **Must contain** (a URL,
a hashtag, a phrase) is added when missing, and the reply is shortened to fit X's length.

Pace, per automation:

- a random pause between actions, `min`–`max` ms (100–5000 by default);
- at most 1 to N actions per 10 minutes: the cap is drawn anew for every 10-minute window;
- daily caps on likes and replies, counted per sign-in across all automations using it;
- at most N posts per account or keyword each run.

What each sign-in did is in `engage/<profile>.json` (posts acted on, the last actions); an agent's
replies are in `engage/runs/<run>/`. X limits automated likes and replies, and an account can be
restricted: keep the caps low.

## Tests

```sh
node --test plugins/x-feed/test/*.test.mjs
```
