# Changelog

All notable changes to this project will be documented in this file.

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.2](https://github.com/noirbizarre/gh-ship/compare/0.2.1..0.2.2) - 2026-08-03

### 🐛 Bug Fixes

- **release** Reuse an existing publish run instead of dispatching a new one - ([c375222](https://github.com/noirbizarre/gh-ship/commit/c375222ec8307725f0a68c1de8aec3a795626784))

## [0.2.1](https://github.com/noirbizarre/gh-ship/compare/0.2.0..0.2.1) - 2026-08-03

### 🐛 Bug Fixes

- **render** Stop the artifact payload closing its own HTML comment - ([2dadfeb](https://github.com/noirbizarre/gh-ship/commit/2dadfebe69a1efc7f7b76ed8dc1e1798c3828618))

### 🔧 CI

- **codecov** Wait for every matrix leg before judging coverage - ([80464eb](https://github.com/noirbizarre/gh-ship/commit/80464eb9a73471a06d7461b31c63cf89a35e425a))

## [0.2.0](https://github.com/noirbizarre/gh-ship/compare/0.1.0..0.2.0) - 2026-08-02

### 💫 Features

- **gh** Retry transient failures on read-only calls - ([93f547d](https://github.com/noirbizarre/gh-ship/commit/93f547d7bf4975abed0a90944206c33692e69567))
- **init** Use the shorter `$schema:` modeline - ([8774edf](https://github.com/noirbizarre/gh-ship/commit/8774edf3aa786119a06d511261f2f98b4cc4ab3c))

### 🐛 Bug Fixes

- **init** Resolve the picked prepare workflow instead of panicking - ([3f06f1b](https://github.com/noirbizarre/gh-ship/commit/3f06f1bc9e60e69b43f04cb128fc189009fe5e2a))
- **prepare** Skip when the published release is still the base tip - ([22e0b80](https://github.com/noirbizarre/gh-ship/commit/22e0b808989ba8e7dc749af2242208a572d4cad4))

### 🔨 Refactor

- **config** Reach settings through their sub-struct - ([5bb80b8](https://github.com/noirbizarre/gh-ship/commit/5bb80b826ef1660f992bdd3b9d8bef7b00d0e402))
- **context** Route every dispatch through one helper - ([fdec880](https://github.com/noirbizarre/gh-ship/commit/fdec8809ede5ebaf4bb9aaf7e914781e5c64bc7e))
- **errors** Enumerate the artifact and release failures - ([af68b78](https://github.com/noirbizarre/gh-ship/commit/af68b786220be095b3dafb35d2315153c2efd6a2))
- **logger** Route every user-facing line through the logger - ([a3834c6](https://github.com/noirbizarre/gh-ship/commit/a3834c6382a2ad81915e880daea0dbefa26e6143))
- **logger** Report the release identity from one place - ([bb27b00](https://github.com/noirbizarre/gh-ship/commit/bb27b001d9e1ea48fa8fffb25abc8d3c5a742a82))
- **run** Name the run timeout after the env var it reads - ([328bc3a](https://github.com/noirbizarre/gh-ship/commit/328bc3ad8c4fd0e75b0445aa2ebdfc00b47e18d4))
- **span** Name the span helpers after what they span - ([bc66a1d](https://github.com/noirbizarre/gh-ship/commit/bc66a1df73170badafb47256ca3409f99a5c3da9))
- Share the workflow label, the plural and the Path import - ([fc7bec9](https://github.com/noirbizarre/gh-ship/commit/fc7bec9fbaf54539cc482b659355fdc1dbbb048b))
- Single-source the duplicated defaults and PR state checks - ([51e6f2a](https://github.com/noirbizarre/gh-ship/commit/51e6f2a58034662d88be025a03767a4b4f6df573))

### 📚 Documentation

- **protocol** Make the artifact spec match what is enforced - ([8607027](https://github.com/noirbizarre/gh-ship/commit/860702710822e5865aacdc10b0117e76b9b44649))
- Settle on one product name, one emoji and one toolchain story - ([8586f50](https://github.com/noirbizarre/gh-ship/commit/8586f50a46e5774961f56ca06a5882e1e0a8ecc7))
- Correct the publish example, the prepare sequence and the tagline - ([f91c895](https://github.com/noirbizarre/gh-ship/commit/f91c89565be8010dd0a8ab2155e662a5be62e6ab))

### 🧪 Tests

- Cover the renderers the refactor introduced - ([6b9b02b](https://github.com/noirbizarre/gh-ship/commit/6b9b02be7a668d92de4bdbe52d1ba42538b39c52))
- Assert what the tests claim to assert - ([3e48330](https://github.com/noirbizarre/gh-ship/commit/3e48330ac818db0798819a98a1b789202605a51e))

### 🔧 CI

- **changelog** Keep the version heading out of the release notes - ([42346fa](https://github.com/noirbizarre/gh-ship/commit/42346faad366e793b79bbea7587e570b0d5368db))
- **github** Use Ship PAT when possible to avoid rate limiting - ([d388b2d](https://github.com/noirbizarre/gh-ship/commit/d388b2d507a0efcf6b8bed5175a87702463c3eb6))
- **release** Name the GitHub Release by version alone - ([52a8ae1](https://github.com/noirbizarre/gh-ship/commit/52a8ae1ca6dec7a339f8697af936770d153acec9))
- **ship** Run the published extension instead of building from source - ([74b4271](https://github.com/noirbizarre/gh-ship/commit/74b4271a60c87297c12a6a077f323e735b445f19))

## 0.1.0 - 2026-08-02

### 💫 Features

- **config** Publish a JSON Schema for .github/ship.yml - ([903ee23](https://github.com/noirbizarre/gh-ship/commit/903ee236c2bea0fe53422c534335fc2699c5b2b4))
- **ship** Release when the Release PR merges - ([6892cbd](https://github.com/noirbizarre/gh-ship/commit/6892cbdf660cb41f5927187ed72c5e2e84568e65))
- **validate** Check that the prepare workflow accepts dry_run - ([cbc1fbc](https://github.com/noirbizarre/gh-ship/commit/cbc1fbc386b36e2b4c69db7a17492416d03de8d2))
- Initial implementation of the GitHub Release Orchestrator - ([6963481](https://github.com/noirbizarre/gh-ship/commit/69634814a8921f423ac57859befd2d44ae626428))

### 🐛 Bug Fixes

- **ci** Repair prepare-release and the docs build, scope release env - ([cf77533](https://github.com/noirbizarre/gh-ship/commit/cf7753304d555bafc3a65857b6bb88d476bae1de))
- **gh** Stop sending --repo to gh api - ([dd7c215](https://github.com/noirbizarre/gh-ship/commit/dd7c2155102c9fd7cb78815b5fe3ac5f3fe422c6))
- **init** Give filesystem failures a code and help text - ([3ca350f](https://github.com/noirbizarre/gh-ship/commit/3ca350f31e0ff2d5e771b6d48c1413013f6c9e51))
- **prepare** Stage the release commit so the PR is never closed - ([4bcb0f2](https://github.com/noirbizarre/gh-ship/commit/4bcb0f2df2cd07e5d8e585492ce56b928c428a95))
- **prepare** Reset the release branch to base before dispatching - ([c118e3c](https://github.com/noirbizarre/gh-ship/commit/c118e3c8bcfd3a84c8a8ae6267653ac57cc22607))
- **release** Report progress while the publish workflow runs - ([8a414e2](https://github.com/noirbizarre/gh-ship/commit/8a414e270355fe8085423627f37f3e93e1271a93))
- **release** Diagnose a missing tag instead of panicking - ([d9bbb36](https://github.com/noirbizarre/gh-ship/commit/d9bbb36f7a11195cbe9d781139e41ea6ef1cc08c))
- **release** Apply make_latest from the artifact - ([a25bd14](https://github.com/noirbizarre/gh-ship/commit/a25bd14b35dd923371e06cf10568563c3df90d36))
- **release** Create the tag before the draft release - ([d35e1bb](https://github.com/noirbizarre/gh-ship/commit/d35e1bb99ff5d58c6c07927241c39d7eb0524c6f))
- **style** Colour the output in GitHub Actions - ([92ec441](https://github.com/noirbizarre/gh-ship/commit/92ec441cc2a6d393ff1471cadf18d0f5df159813))
- Identify workflows by slug, create missing PR labels, lint workflows - ([25d336f](https://github.com/noirbizarre/gh-ship/commit/25d336f4242333141eaba14f2e98159b1dc82b60))

### 🔨 Refactor

- **commands** Share helpers and drop dead code - ([88314bb](https://github.com/noirbizarre/gh-ship/commit/88314bb0db2098dde48b6d562b354daa4f9c7fa2))

### 📚 Documentation

- **architecture** Explain who creates the tag and the release - ([85b5961](https://github.com/noirbizarre/gh-ship/commit/85b59613129ad2dc64fe5546dafbbbece3d6cd84))
- **cli** Correct the prepare sequence and document the wait timeouts - ([8ff02dd](https://github.com/noirbizarre/gh-ship/commit/8ff02dd5d2ba3791a42771501c6e44f2265b3284))
- **config** Stop claiming the prepare workflow commits to the release branch - ([5f01893](https://github.com/noirbizarre/gh-ship/commit/5f018936febfc4ca4dad154096e6b187f6a7f461))
- **install** Document a single installation method - ([89c121a](https://github.com/noirbizarre/gh-ship/commit/89c121ade053528b9c10ca6c9069e8171f515bd9))
- **logo** Recolour to the two-colour palette and clean the SVGs - ([8782cc3](https://github.com/noirbizarre/gh-ship/commit/8782cc3cb8cf7ced6474941a2876c0764c85ab4a))
- **readme** Add logo and readme header - ([16bec6f](https://github.com/noirbizarre/gh-ship/commit/16bec6fb3633fc87d5c7f21269dddec19bf13747))
- **site** Show the logo and follow the system colour scheme - ([c354eb8](https://github.com/noirbizarre/gh-ship/commit/c354eb8e50f0516b91c5539e6cf0cc8d22d676cb))
- **token** Document the permissions SHIP_TOKEN needs - ([7a69fc2](https://github.com/noirbizarre/gh-ship/commit/7a69fc2293f6598d1d43cc020f785f9f9dd9a756))
- **workflows** Align the examples with what gh-ship actually generates - ([edc6846](https://github.com/noirbizarre/gh-ship/commit/edc6846a6a144bfe3a2e4b289d14b933e3891310))
- Fix the release description, the card icons and the product name - ([7285226](https://github.com/noirbizarre/gh-ship/commit/7285226c18bfee607d1b9b33169fc4362c6a1424))
- Correct sample output and module descriptions - ([583de82](https://github.com/noirbizarre/gh-ship/commit/583de828e74bd69b80ddd694b4aa5287abda0453))

### 🧪 Tests

- **config** Make the schema drift guard actually guard - ([4ca2906](https://github.com/noirbizarre/gh-ship/commit/4ca2906df84568ec08db11f77a624ecc67a75895))
- **gh** Stop mutating PATH to test a missing gh binary - ([a6e641b](https://github.com/noirbizarre/gh-ship/commit/a6e641bdf96f52bd00ae8ef07309063be3a100d3))
- **prepare** Cover the staging-branch sweep - ([485abf0](https://github.com/noirbizarre/gh-ship/commit/485abf07686e802dd1b37f4cc00ff7b3591e94f9))

### 🏗️ Build

- **changelog** Drop the redundant changelog normalisation - ([249e1c3](https://github.com/noirbizarre/gh-ship/commit/249e1c3675a2ff842da71a92cf20de4123b6d8e8))
- **changelog** Adopt the release-requests changelog format - ([c6f3056](https://github.com/noirbizarre/gh-ship/commit/c6f3056e40c8466c7d02cd839603d13ff353e531))
- **icons** Add a mise task to rasterize the icon - ([5458f60](https://github.com/noirbizarre/gh-ship/commit/5458f6091b921c86fe2043d46ee803af70809b90))
- **social** Generate the GitHub social preview from the logo - ([dbc493e](https://github.com/noirbizarre/gh-ship/commit/dbc493e0ba37a01764a602150ec3427c3739ba0b))
- Tag releases without a `v` prefix - ([9cda6e5](https://github.com/noirbizarre/gh-ship/commit/9cda6e5227a2116be7df6515a24bba2eb6d8685e))
- Track mise.toml and mise.lock, and install CI tools from them - ([cc8a2b0](https://github.com/noirbizarre/gh-ship/commit/cc8a2b04c3e7ced2c43e06d29dd73d5d9d2e0512))

### 🔧 CI

- **changelog** Fix first release version (no diff url) in cliff template - ([6cf8489](https://github.com/noirbizarre/gh-ship/commit/6cf8489ffc0c2f7e901dafc6b4325bd22749c3fd))
- **codecov** Add codecov config - ([a9e64e9](https://github.com/noirbizarre/gh-ship/commit/a9e64e92eded11d78aa76daef9bf9f2381a3e296))
- **lint** Install shellcheck and stop generated files from failing lint - ([9497fe9](https://github.com/noirbizarre/gh-ship/commit/9497fe9c054751a005cd95c06735c9039a64e709))
- **release** Stop publishing to crates.io - ([8ddc292](https://github.com/noirbizarre/gh-ship/commit/8ddc29298744035b2c9e73495f47050492631e8a))
- **release** Add the Ship workflow to prepare releases on every push - ([37a630e](https://github.com/noirbizarre/gh-ship/commit/37a630eaf1415c9197e3b6909459929648486d9f))
- **release** Fix `release` environment deployment - ([d275b1d](https://github.com/noirbizarre/gh-ship/commit/d275b1d0ec85835e589b84fe3b00bc438065c05c))

## ❤️ New Contributors

* @noirbizarre made their first contribution in [#4](https://github.com/noirbizarre/gh-ship/pull/4)
