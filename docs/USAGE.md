# Using Minima

## What it does

| | |
|---|---|
| **Strips pages** | A compiled logistic model scores every block (nav, ads, footers, banners vs. real content) from tag, ARIA role, class/id words, link density, sentence density, text length. It also decides which forms matter and what each field/button is for, and classifies the page (article, results, login…). |
| **Everything is usable** | Links, buttons, text boxes, password fields, checkboxes, radio buttons, drop-downs. GET and POST forms work; cookies persist for the session. |
| **Keyboard first** | `Tab`/`Shift+Tab`, arrows, `Enter`. Mouse wheel and clicks work too. |
| **Ask it in plain English** | Press `:` and type `click sign in`, `search for rust`, `type alice in username`, `price`. A compiled synonym-aware matcher finds the control or text. `log in` ≈ `sign in` ≈ `login`, but `sign up` is not `sign in`. |
| **Images** | Select an image, press `Enter` or `i`. Three renderers: `ascii`, `blocks` (half-blocks) and `braille` (8 pixels per cell, dithered). The dense modes keep pictures sharp **without** changing the terminal font. On xterm-compatible terminals Minima also asks the terminal for a smaller font while an image is showing. |
| **Video** | Animated GIFs play with no extra tools. Other video plays through `ffmpeg` (frames → ASCII) with sound via `ffplay`. YouTube search and watch pages are rebuilt as clean pages; playback uses `yt-dlp`. |
| **Downloads** | `d` on a link, image or video (or the current page). Videos use `yt-dlp` when present. |
| **History & bookmarks** | Persistent (`H`, `B`, `b`). Filter by typing. Back/forward keeps your scroll position and is instant (pages stay cached). |

## Keys

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | next / previous link, button or field |
| `↑` `↓` | move to the control above / below (scrolls when there is none) |
| `←` `→` | sideways between controls on one line; at the edge: back / forward |
| `Enter` | follow, press, edit field, toggle, open menu. A selected text box also takes typing directly |
| `Space` `PgUp` `PgDn` `Home` `End` `j` `k` | scroll |
| `g` / `G` | go to URL or search / edit current URL |
| `:` | ask Minima in plain English |
| `/` `n` `N` | find in page |
| `L` | list every link and control (filterable) |
| `i` `I` | view image / list images |
| `p` | play video |
| `d` | download |
| `H` `B` `b` | history / bookmarks / bookmark this page |
| `r` | reload |
| `?` | help |
| `q` / `Ctrl+C` | quit |

In the image viewer: `m` cycles ascii → blocks → braille, `c` toggles colour, `-`/`0` zoom,
`←`/`→` previous/next image, `d` saves, `f` asks the terminal for a smaller font.

## Honest limitations

* **No JavaScript.** Pages that render entirely client-side show only what the server sends.
  Minima says so and shows the `<noscript>` content and page description. Many sites (news,
  docs, wikis, forums, search engines' HTML endpoints, Hacker News) work very well.
* **Google** often refuses to serve results to non-JavaScript clients. Minima detects that and
  transparently retries the same query on DuckDuckGo's HTML endpoint, saying so on the page.
  Change `search` in the config file to use any engine.
* **The "NLP" is a compiled linear model + lexicon**, not a large language model. It is fast,
  tiny and deterministic; the weights live in `crates/minima/model/` and are compiled in by
  `build.rs`. Retrain or tweak them and rebuild.
* **Video**: decoding H.264/VP9 in pure Rust is not practical, so video needs `ffmpeg`
  (and `ffplay` for sound). The installers can fetch both (`--with-media`). YouTube needs
  `yt-dlp` too. Everything else works without them.


## Configuration

`minima --config` prints the path of the config file (created on first run):

```
home = https://www.google.com/
search = https://www.google.com/search?q={q}
image_mode = braille      # ascii | blocks | braille
color = true
font_shrink = false       # ask xterm-like terminals for a smaller font in image/video view
video_fps = 12
```

Data (history, bookmarks) lives in your OS data directory under `minima/`.

