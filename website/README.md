# Dapper Website

This website is built with [Docusaurus](https://docusaurus.io/).

## Development

If running on EdenFS, redirect generated directories first:

```bash
eden redirect add fbcode/dapper/website/node_modules bind
eden redirect add fbcode/dapper/website/.docusaurus bind
eden redirect add fbcode/dapper/website/build bind
```

Then install and start:

```bash
cd fbcode/dapper/website
yarn install
yarn start          # external/OSS mode
yarn start-internal # internal mode
```

The site will be available at `http://localhost:3000/`.

## Docs

Most documentation pages are sourced from the built-in help topics in
`dapper_cli/src/help/topics/`. Docusaurus prepares them into `.generated/docs/`
when it loads the site config, with `{{program}}` token substitution.

The help topics are the single source of truth — edit them there, not in
`.generated/docs/`. Only `docs/index.md` is owned by the website.

Internal-only docs are copied from `fb/dapper_fb_main/src/help/topics/` only
when `INTERNAL_STATIC_DOCS=1` is set.

## Build

```bash
yarn build
```
