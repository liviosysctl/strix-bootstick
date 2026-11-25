# The window and its assets

`theme.slint`, `parts.slint` and `app.slint` are written for this app. The values in `theme.slint` are those of [StrixUI](https://github.com/liviosysctl/StrixUI) (`design-system/tokens.json`, theme Saturnus) at commit `31f3d42`; the components in `parts.slint` follow StrixUI's Button, Progress, the steps of the shell and its smoked glass, with the same measures.

| Here | From | Licence |
|---|---|---|
| `fonts/HankenGrotesk-*.ttf`, `fonts/CormorantGaramond-*.ttf`, `fonts/IBMPlexMono-Regular.ttf` | StrixUI `src/fonts/`, turned from WOFF2 into TrueType, because Slint reads TrueType | SIL Open Font License, the licence files are next to them |
| `images/saturnus.jpg` | StrixUI `design-system/assets/Wallpapers/`: Jacob Binck after Giovanni Jacopo Caraglio after Rosso Fiorentino, Saturnus, from the Gods in Niches, 1530. Rijksmuseum RP-P-1951-178A | public domain |
| `images/owl.svg` | StrixUI `design-system/assets/Logo/strix-owl-small-saturnus.svg`, the owl of Strix | own drawing |
| `icons/*.svg` | StrixUI `src/icons/` (Lucide), with the stroke of StrixUI (1.5) and a fixed white for Slint to tint | ISC, `icons/LICENSE-Lucide.txt` |
| `../icons/icon.ico` | the owl on the dark ground, rendered once | own drawing |

The variable Cormorant Garamond names its family after its default instance, so it is `Cormorant Garamond Light` in `theme.slint`.

Slint itself is used under its royalty-free licence, which asks for its badge in an About box: the "i" in the title bar opens it.
