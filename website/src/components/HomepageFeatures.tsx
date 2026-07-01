/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

import React from 'react';
import clsx from 'clsx';
import styles from './HomepageFeatures.module.css';

type Feature = {
  icon: React.ReactNode;
  title: string;
  description: string;
};

const features: Feature[] = [
  {
    icon: (
      <svg viewBox="0 0 24 24" focusable="false">
        <path d="M7 3.75h10a2.25 2.25 0 0 1 2.25 2.25v12A2.25 2.25 0 0 1 17 20.25H7A2.25 2.25 0 0 1 4.75 18V6A2.25 2.25 0 0 1 7 3.75Z" />
        <path d="m9 8 2.75 4L9 16" />
        <path d="M13.75 16h3.25" />
      </svg>
    ),
    title: 'Debugger access for agents',
    description:
      'Give Claude, Copilot, or any MCP client full access to breakpoints, stepping, variables, and eval.',
  },
  {
    icon: (
      <svg viewBox="0 0 24 24" focusable="false">
        <path d="M8.5 11.25a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z" />
        <path d="M15.5 11.25a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z" />
        <path d="M4.75 18.75a4.75 4.75 0 0 1 7.25-4.03 4.75 4.75 0 0 1 7.25 4.03" />
      </svg>
    ),
    title: 'Debug together',
    description:
      'Human in the IDE, agent in the terminal, same live session.',
  },
  {
    icon: (
      <svg viewBox="0 0 24 24" focusable="false">
        <path d="M12 3.75 18.25 7.25v6.25c0 3.5-2.35 5.65-6.25 6.75-3.9-1.1-6.25-3.25-6.25-6.75V7.25L12 3.75Z" />
        <path d="M9 12.25h6" />
        <path d="M12 9.25v6" />
      </svg>
    ),
    title: 'Autonomous debugging',
    description:
      'Point an agent at a crash dump or failing test and let it debug without human involvement.',
  },
  {
    icon: (
      <svg viewBox="0 0 24 24" focusable="false">
        <path d="M12 4.75v3.5" />
        <path d="M12 15.75v3.5" />
        <path d="M4.75 12h3.5" />
        <path d="M15.75 12h3.5" />
        <path d="M8.25 8.25 5.75 5.75" />
        <path d="m18.25 18.25-2.5-2.5" />
        <path d="m15.75 8.25 2.5-2.5" />
        <path d="m5.75 18.25 2.5-2.5" />
        <circle cx="12" cy="12" r="3.75" />
      </svg>
    ),
    title: 'Works with your stack',
    description:
      'Dapper proxies existing DAP adapters like lldb-dap, debugpy, and dlv. No new debugger to learn.',
  },
];

function Feature({icon, title, description}: Feature) {
  return (
    <div className={clsx('col col--6', styles.feature)}>
      <div className={styles.featureCard}>
        <div className={styles.featureHeader}>
          <span className={styles.featureIcon} aria-hidden="true">
            {icon}
          </span>
          <h3 className={styles.featureTitle}>{title}</h3>
        </div>
        <p className={styles.featureDescription}>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures(): React.ReactElement {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {features.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
