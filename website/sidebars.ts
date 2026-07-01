/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

import {isInternal} from 'docusaurus-plugin-internaldocs-fb/internal';
import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docs: [
    'index',
    'installation',
    'getting-started',
    'ide-workflow',
    'ai-agent',
    'mcp',
    ...(isInternal()
      ? [
          {
            type: 'category' as const,
            label: 'Debuggers',
            collapsed: false,
            items: [
              'reference/fb/debuggers',
              'reference/fb/debuggers/lldb',
              'reference/fb/debuggers/python',
            ],
          },
        ]
      : []),
    {
      type: 'category',
      label: 'Reference',
      collapsed: true,
      items: [
        'reference/overview',
        'reference/mcp',
        'reference/sessions',
        'reference/breakpoints',
        'reference/agent',
        {
          type: 'category',
          label: 'CLI',
          collapsed: false,
          items: ['reference/debug', 'reference/proxy'],
        },
        ...(isInternal()
          ? [
              {
                type: 'doc' as const,
                id: 'reference/fb/headless',
                label: 'Headless Mode Entry Points',
              },
            ]
          : []),
      ],
    },
  ],
};

export default sidebars;
