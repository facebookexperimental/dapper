/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

// Our internal doc builder requires a `.js` file to exist, so have this and
// keep the actual implementation in `.ts`
// docusaurus-plugin-internaldocs-fb/internal

const {config} = require('./config_impl.ts');

module.exports = config;
