/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

import {themes} from 'prism-react-renderer';
import {
  fbContent,
  isInternal,
} from 'docusaurus-plugin-internaldocs-fb/internal';
import type {
  ThemeConfig as ClassicPresetConfig,
  Options as ClassicPresetOptions,
} from '@docusaurus/preset-classic';
import type {DocusaurusConfig} from '@docusaurus/types';
import {prepareHelpDocs} from './helpDocs';

const lightCodeTheme = themes.github;
const darkCodeTheme = themes.dracula;
const docsPath = prepareHelpDocs();
const sourceCodeUrl = fbContent({
  internal: 'https://www.internalfb.com/code/fbsource/fbcode/dapper/',
  external: 'https://github.com/facebookexperimental/dapper',
});
const sourceCodeLabel = fbContent({
  internal: 'CodeHub',
  external: 'GitHub',
});

const presetOptions: ClassicPresetOptions = {
  docs: {
    path: docsPath,
    sidebarPath: require.resolve('./sidebars.ts'),
  },
  blog: false,
  theme: {
    customCss: require.resolve('./src/css/custom.css'),
  },
  staticDocsProject: 'dapper',
  internSearch: true,
};

const themeConfig: ClassicPresetConfig = {
  colorMode: {
    defaultMode: 'light',
    respectPrefersColorScheme: true,
  },
  docs: {
    sidebar: {
      hideable: true,
    },
  },
  navbar: {
    logo: {
      alt: 'Dapper',
      src: 'img/logo.svg',
      srcDark: 'img/logo-dark.svg',
    },
    items: [
      {
        type: 'doc',
        docId: 'index',
        position: 'left',
        label: 'Docs',
      },
      {
        type: 'doc',
        docId: 'reference/overview',
        position: 'left',
        label: 'References',
      },
      {
        href: sourceCodeUrl,
        // @ts-ignore
        label: sourceCodeLabel,
        position: 'right',
      },
    ],
  },
  footer: {
    style: 'dark',
    links: [
      {
        title: 'Docs',
        items: [
          {
            label: 'Getting Started',
            to: '/docs',
          },
        ],
      },
      {
        title: 'Community',
        items: isInternal()
          ? [
              {
                label: 'Workplace Group',
                href: 'https://fb.workplace.com/groups/dapper.eng',
              },
            ]
          : [
              {
                label: 'GitHub Issues',
                href: 'https://github.com/facebookexperimental/dapper/issues',
              },
            ],
      },
      {
        title: 'More',
        items: [
          {
            label: 'Code',
            href: sourceCodeUrl,
          },
          {
            label: 'Terms of Use',
            href: 'https://opensource.fb.com/legal/terms',
          },
          {
            label: 'Privacy Policy',
            href: 'https://opensource.fb.com/legal/privacy',
          },
        ],
      },
    ],
    copyright: `Copyright © ${new Date().getFullYear()} Meta Platforms, Inc. Built with Docusaurus.`,
  },
  prism: {
    additionalLanguages: ['bash', 'json', 'toml', 'rust', 'python'],
    theme: lightCodeTheme,
    darkTheme: darkCodeTheme,
  },
};

const config: DocusaurusConfig = {
  title: 'Dapper',
  tagline: 'Let AI agents debug your code.',
  url: 'https://dapper-debug.dev',
  baseUrl: '/',
  onBrokenLinks: 'throw',
  trailingSlash: true,
  favicon: 'img/favicon-lens.svg',
  organizationName: 'facebookexperimental',
  projectName: 'dapper',
  customFields: {
    sourceCodeUrl,
    sourceCodeLabel,
  },

  presets: [
    [
      require.resolve('docusaurus-plugin-internaldocs-fb/docusaurus-preset'),
      presetOptions,
    ],
  ],

  themeConfig,

  markdown: {
    format: 'detect',
    mermaid: true,
  },
  themes: ['@docusaurus/theme-mermaid'],
};

module.exports = {
  config,
};
