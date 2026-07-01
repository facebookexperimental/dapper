/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

import fs from 'node:fs';
import path from 'node:path';

const WEBSITE_DIR = __dirname;
const DAPPER_DIR = path.dirname(WEBSITE_DIR);
const GENERATED_DOCS_DIR = path.join(WEBSITE_DIR, '.generated', 'docs');
const WEBSITE_DOCS_DIR = path.join(WEBSITE_DIR, 'docs');
const REFERENCE_DOCS_DIR = path.join(GENERATED_DOCS_DIR, 'reference');
const OSS_TOPICS_DIR = path.join(
  DAPPER_DIR,
  'dapper_cli',
  'src',
  'help',
  'topics',
);
const FB_TOPICS_DIR = path.join(
  DAPPER_DIR,
  'fb',
  'dapper_fb_main',
  'src',
  'help',
  'topics',
);
const INCLUDE_INTERNAL_DOCS = process.env.INTERNAL_STATIC_DOCS === '1';
const PROGRAM_NAME = INCLUDE_INTERNAL_DOCS ? 'fdb dapper' : 'dapper';

const TOOLSET_TABLE = `| Toolset | Tools | Description |
|---------|-------|-------------|
| \`minimal\` | status, threads, stack-trace, scopes, variables, capabilities | Read-only inspection |
| \`standard\` *(default)* | All minimal + navigate, set-breakpoints, set-exception-breakpoints, stop | Adds navigation and breakpoints |
| \`full\` | All standard + evaluate, set-variable, read-memory, write-memory, thread-snapshot | All tools including memory access |
| \`raw\` | dap-request, stop | Single pass-through tool for any DAP command |`;

const REFERENCE_FRONTMATTER: Record<string, () => string> = {
  'agent.md': () => 'title: Agent Operating Guide\nsidebar_label: Agent Guide',
  'breakpoints.md': () => 'title: Breakpoints\nsidebar_label: Breakpoints',
  'debug.md': () => `title: ${PROGRAM_NAME} debug\nsidebar_label: debug`,
  'mcp.md': () => `title: ${PROGRAM_NAME} mcp\nsidebar_label: MCP`,
  'overview.md': () => 'title: CLI Help Overview\nsidebar_label: Overview',
  'proxy.md': () => `title: ${PROGRAM_NAME} proxy\nsidebar_label: proxy`,
  'sessions.md': () => 'title: Sessions\nsidebar_label: Sessions',
};
const INTERNAL_OVERRIDES = [
  'ai-agent.md',
  'getting-started.md',
  'ide-workflow.md',
  'installation.md',
  'mcp.md',
];

export function prepareHelpDocs(): string {
  fs.rmSync(GENERATED_DOCS_DIR, {recursive: true, force: true});
  fs.mkdirSync(GENERATED_DOCS_DIR, {recursive: true});

  copyWebsiteDocs(WEBSITE_DOCS_DIR, GENERATED_DOCS_DIR);
  copyInternalOverrides();
  copyMarkdownFiles(OSS_TOPICS_DIR, REFERENCE_DOCS_DIR);

  if (INCLUDE_INTERNAL_DOCS && fs.existsSync(FB_TOPICS_DIR)) {
    copyMarkdownFiles(FB_TOPICS_DIR, path.join(REFERENCE_DOCS_DIR, 'fb'));
    copyMarkdownFiles(
      path.join(FB_TOPICS_DIR, 'debuggers'),
      path.join(REFERENCE_DOCS_DIR, 'fb', 'debuggers'),
    );
  }

  fs.rmSync(path.join(WEBSITE_DIR, 'build', 'rendered-components'), {
    recursive: true,
    force: true,
  });

  return './.generated/docs';
}

function copyWebsiteDocs(sourceDir: string, targetDir: string): void {
  fs.mkdirSync(targetDir, {recursive: true});
  fs.readdirSync(sourceDir, {withFileTypes: true}).forEach(entry => {
    if (entry.name === 'fb') {
      return;
    }

    const source = path.join(sourceDir, entry.name);
    const target = path.join(targetDir, entry.name);
    if (entry.isDirectory()) {
      copyWebsiteDocs(source, target);
      return;
    }

    if (entry.isFile()) {
      fs.copyFileSync(source, target);
    }
  });
}

function copyInternalOverrides(): void {
  if (!INCLUDE_INTERNAL_DOCS) {
    return;
  }

  INTERNAL_OVERRIDES.forEach(fileName => {
    const override = path.join(WEBSITE_DOCS_DIR, 'fb', fileName);
    if (fs.existsSync(override)) {
      fs.copyFileSync(override, path.join(GENERATED_DOCS_DIR, fileName));
    }
  });
}

function copyMarkdownFiles(sourceDir: string, targetDir: string): void {
  if (!fs.existsSync(sourceDir)) {
    return;
  }

  fs.mkdirSync(targetDir, {recursive: true});
  fs.readdirSync(sourceDir, {withFileTypes: true})
    .filter(entry => entry.isFile() && entry.name.endsWith('.md'))
    .forEach(entry => {
      const source = path.join(sourceDir, entry.name);
      const target = path.join(targetDir, entry.name);
      fs.writeFileSync(target, renderHelpTopic(source));
    });
}

function renderHelpTopic(source: string): string {
  const body = fs
    .readFileSync(source, 'utf8')
    .split('{{program}}')
    .join(PROGRAM_NAME)
    .split('{{toolset_table}}')
    .join(TOOLSET_TABLE);
  const frontmatter = REFERENCE_FRONTMATTER[path.basename(source)];
  return frontmatter == null ? body : `---\n${frontmatter()}\n---\n\n${body}`;
}
