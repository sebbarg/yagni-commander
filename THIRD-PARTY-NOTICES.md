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

## JetBrains Mono

`crates/yagni-commander/assets/fonts/JetBrainsMonoNL-Regular.ttf`, from JetBrains Mono v2.304 (https://github.com/JetBrains/JetBrainsMono), unchanged. Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono). SIL Open Font License 1.1 (`crates/yagni-commander/assets/fonts/JetBrainsMono-OFL.txt`).

## nvim-web-devicons

`crates/yagni-commander-core/src/icons/table.rs` is generated from nvim-web-devicons v0.100 (https://github.com/nvim-tree/nvim-web-devicons) by `scripts/gen-icons.py`. MIT License, Copyright (c) 2023 nvim-tree.

## Highlight queries

`crates/yagni-commander/assets/queries/` holds tree-sitter highlight queries that the F3 viewer's source mode registers in place of gpui-component's:

| File | Source | License |
|---|---|---|
| `csharp-highlights.scm` | tree-sitter-c-sharp 0.23.5 (https://github.com/tree-sitter/tree-sitter-c-sharp), `queries/highlights.scm`, unchanged. Copyright (c) 2014-2023 Max Brunsfeld, Damien Guard, Amaan Qureshi, and contributors | MIT |
| `proto-highlights.scm` | tree-sitter-proto 0.2.0 (https://github.com/coder3101/tree-sitter-proto), `queries/highlights.scm`, unchanged | MIT |
| `swift-highlights.scm` | tree-sitter-swift 0.7.4 (https://github.com/alex-pinkus/tree-sitter-swift), `queries/highlights.scm`, unchanged. Copyright (c) 2021 alex-pinkus | MIT |
| `kotlin-highlights.scm` | gpui-component 0.7.0 (https://github.com/longbridge/gpui-kit), `src/highlighter/languages/kotlin/highlights.scm`. Modified: four patterns the grammar has no nodes for are left out (see the file's header) | Apache 2.0 |
