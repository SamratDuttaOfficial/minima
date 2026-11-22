# FAQ and troubleshooting

## A page comes up nearly empty
Minima has no JavaScript engine. If a site builds its content in the browser, the server sends
almost nothing and that is all Minima can show. It displays the page description and any
`<noscript>` text so you at least know why. Sites that serve real HTML (news, docs, wikis,
forums, Hacker News) work well.

## Google gives me different results
Google often refuses to serve results to clients without JavaScript. Minima notices and retries
the same query on DuckDuckGo's HTML endpoint, and says so at the top of the page. To use another
engine, change `search` in the config file (`minima --config` prints where it is).

## It says it needs an interactive terminal
Minima's full interface needs a real terminal. For scripts and pipes use
`minima --dump URL` (prints the cleaned page as text) or `minima --image FILE` (prints art).

## Images look blocky or too small
Press `m` in the image viewer to cycle `ascii`, `blocks` and `braille`. `braille` is the sharpest.
`-` and `0` zoom. On xterm-compatible terminals, `f` asks the terminal for a smaller font while
the image is showing (or set `font_shrink = true`).

## Video does nothing
Animated GIFs play on their own. Other video needs `ffmpeg` (and `ffplay` for sound). YouTube also
needs `yt-dlp`. The installers can fetch ffmpeg and ffplay with `--with-media`.

## How does it decide what to hide?
A small scoring model compiled into the binary rates every block of the page (tag, ARIA role,
class and id words, link density, sentence density, text length). It is not a large language
model and it never leaves your machine. The weights are text files in `crates/minima/model/`,
so you can read them, change them and rebuild.

## Where are my history and bookmarks?
In your OS data directory under `minima/`. `H` opens history, `B` bookmarks, `b` bookmarks the
current page.

## How do I uninstall?
Run the CLI installer again with `--uninstall`, or use your system's usual uninstall (Windows
Apps list, `apt remove minima`, `dnf remove minima`).

## It is wrong about something
Open an issue with the URL and what you expected. Minima is a one-person project, so a concrete
example helps a lot.
