# Third-party notices

yagni-commander includes the following third-party material.

## Symbols Nerd Font Mono

`crates/yagni-commander/assets/fonts/SymbolsNerdFontMono-Regular.ttf`, from Nerd Fonts v3.5.1 (https://github.com/ryanoasis/nerd-fonts). Copyright (c) 2014 Ryan L McIntyre, MIT License (`crates/yagni-commander/assets/fonts/LICENSE`).

Modified: the character `m` is mapped to the font's empty `nonmarkingreturn` glyph, because gpui's Linux text system ignores fonts without an `m`. `scripts/patch-icon-font.py` makes this change from the release archive. No glyph was added, removed or changed.

The font contains glyphs from these icon sets, each under its own license:

| Icon set | Source | License |
|---|---|---|
| Codicons | https://github.com/microsoft/vscode-codicons | CC BY 4.0 |
| Devicons | https://github.com/devicons/devicon | MIT |
| Font Awesome | https://github.com/FortAwesome/Font-Awesome | CC BY 4.0 |
| Font Awesome Extension | https://github.com/AndreLZGava/font-awesome-extension | MIT |
| Font Logos | https://github.com/lukas-w/font-logos | Unlicense |
| Material Design Icons | https://github.com/Templarian/MaterialDesign-Font | Apache 2.0 |
| Octicons | https://github.com/primer/octicons | MIT |
| Seti UI | https://github.com/jesseweed/seti-ui | MIT |
| Pomicons | https://github.com/gabrielelana/pomicons | OFL 1.1 |
| Powerline Extra Symbols | https://github.com/ryanoasis/powerline-extra-symbols | MIT |
| Powerline Symbols | https://github.com/powerline/powerline | MIT |
| IEC Power Symbols | https://github.com/jloughry/Unicode | MIT |
| Weather Icons | https://github.com/erikflowers/weather-icons | OFL 1.1 |

## nvim-web-devicons

`crates/yagni-commander-core/src/icons/table.rs` is generated from nvim-web-devicons v0.100 (https://github.com/nvim-tree/nvim-web-devicons) by `scripts/gen-icons.py`. MIT License, Copyright (c) 2023 nvim-tree.
