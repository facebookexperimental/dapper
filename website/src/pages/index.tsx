/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

import React from 'react';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import HomepageFeatures from '../components/HomepageFeatures';
import styles from './index.module.css';

function HomepageHeader() {
  const {siteConfig} = useDocusaurusContext();
  const sourceCodeUrl = siteConfig.customFields?.sourceCodeUrl;
  const sourceCodeLabel = siteConfig.customFields?.sourceCodeLabel;
  const sourceCodeLink =
    typeof sourceCodeUrl === 'string' && typeof sourceCodeLabel === 'string'
      ? {label: sourceCodeLabel, url: sourceCodeUrl}
      : null;

  return (
    <header className={clsx('hero', styles.heroBanner)}>
      <div className="container">
        <h1 className={clsx('hero__title', styles.heroTitle)}>
          {siteConfig.title.toLowerCase()}
        </h1>
        <p className={clsx('hero__subtitle', styles.heroSubtitle)}>
          {siteConfig.tagline}
        </p>
        <div className={styles.buttons}>
          <Link
            className={clsx('button button--lg', styles.heroButtonPrimary)}
            to="/docs/">
            Get Started
          </Link>
          <Link
            className={clsx('button button--lg', styles.heroButton)}
            to="/docs/mcp">
            MCP Server
          </Link>
          {sourceCodeLink && (
            <Link
              className={clsx('button button--lg', styles.heroButton)}
              href={sourceCodeLink.url}>
              {sourceCodeLink.label}
            </Link>
          )}
        </div>
      </div>
    </header>
  );
}

export default function Home(): React.ReactElement {
  const {siteConfig} = useDocusaurusContext();
  return (
    <Layout
      title={siteConfig.title}
      description="Dapper is a DAP proxy that lets AI agents, IDEs, and CLI tools share a single debug session.">
      <HomepageHeader />
      <main>
        <HomepageFeatures />
      </main>
    </Layout>
  );
}
