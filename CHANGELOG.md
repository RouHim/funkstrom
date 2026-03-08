## [1.0.6](https://github.com/RouHim/funkstrom/compare/1.0.5...1.0.6) (2026-03-08)


### Bug Fixes

* harden ffmpeg path resolution for container runtime ([58040cd](https://github.com/RouHim/funkstrom/commit/58040cd3723aa927b64515b127abb19c324f2e7e))

## [1.0.5](https://github.com/RouHim/funkstrom/compare/1.0.4...1.0.5) (2026-03-08)


### Bug Fixes

* remove unused url field from StationConfig ([58655b4](https://github.com/RouHim/funkstrom/commit/58655b49d803ddb661245c86a756678543b9704b))

## [1.0.4](https://github.com/RouHim/funkstrom/compare/1.0.3...1.0.4) (2026-03-08)


### Bug Fixes

* **ci:** publish container images to ghcr ([2b49dd0](https://github.com/RouHim/funkstrom/commit/2b49dd0617bf269674c044e29f5d1eb8071d3b67))

## [1.0.3](https://github.com/RouHim/funkstrom/compare/1.0.2...1.0.3) (2026-03-08)


### Bug Fixes

* **ci:** use docker buildx for image publish ([e917daa](https://github.com/RouHim/funkstrom/commit/e917daa595035c48ae627132b3161dc9edd607cf))

## [1.0.2](https://github.com/RouHim/funkstrom/compare/1.0.1...1.0.2) (2026-03-08)


### Bug Fixes

* **ci:** make container publish resilient and deterministic ([56ceb91](https://github.com/RouHim/funkstrom/commit/56ceb91d6b216f65e53fd97260ee4806f6603778))

## [1.0.1](https://github.com/RouHim/funkstrom/compare/1.0.0...1.0.1) (2026-03-08)


### Bug Fixes

* **ci:** make container build work in GitHub Actions ([05ed75f](https://github.com/RouHim/funkstrom/commit/05ed75f8b05a0ea10024f1e7188774f3178923e5))

# 1.0.0 (2026-03-04)


### Bug Fixes

* **audio:** add FFmpeg stderr capture, exponential backoff, and log level adjustments to prevent log spam ([16adf95](https://github.com/RouHim/funkstrom/commit/16adf953f4c59ca8a07a9247c951531b4ad8022e))
* **audio:** map format names to correct FFmpeg muxers, fixing broken AAC stream ([6832bdc](https://github.com/RouHim/funkstrom/commit/6832bdc5eaa0cb8125228dba0e58625ba5005b64))
* **ci:** fix tag detection and add build-container main branch guard ([a39cac9](https://github.com/RouHim/funkstrom/commit/a39cac9970edf724bfb3578b8ef08b4fe71eb6ab))
* **ci:** grant release token permissions and skip docker publish without creds ([e9a3713](https://github.com/RouHim/funkstrom/commit/e9a37130531a200f3e7d62a292325957162d0461))
* **ci:** make e2e startup wait for stream readiness ([566ff17](https://github.com/RouHim/funkstrom/commit/566ff17f81eae838a12a5430691363f231490c0b))
* **ci:** stabilize e2e by priming stream before test script ([fa226eb](https://github.com/RouHim/funkstrom/commit/fa226eb3bbec8b68e1bdc1400d71d6e9007f39c1))
* **e2e:** align status assertions with current API schema ([5c4d593](https://github.com/RouHim/funkstrom/commit/5c4d5935cc1179301df3cadbc079a04529708b1b))
* improve CI trigger conditions and branch guards ([bb50644](https://github.com/RouHim/funkstrom/commit/bb50644494a628b2e18b8692362897df2f1a035f))
* resolve panics, improve error handling, and remove dead code across codebase ([534eace](https://github.com/RouHim/funkstrom/commit/534eace6fac606b32b73d1069c446284493eebc3))
* **web:** consolidate font-size values to 3 distinct sizes (1rem, 0.8rem, 1.5rem) ([fcdf32f](https://github.com/RouHim/funkstrom/commit/fcdf32ff7a9f69b085c7770febfccf5f519bc32a))
* **web:** normalize html base font-size to 1rem for consistent scale ([14d6fa6](https://github.com/RouHim/funkstrom/commit/14d6fa67aa4d99e6002f4126140393b1fa27744c))


### Features

* add Containerfile and update README with config volume mount ([acf37de](https://github.com/RouHim/funkstrom/commit/acf37def721fb5658a5a193d6db4a70999cf2a66))
* initial implementation of Funkstrom internet radio server ([81ce73d](https://github.com/RouHim/funkstrom/commit/81ce73d886c7ba78c8f61e31c72f5b4ae977b2d4))
* **web:** redesign UI with functional minimalism aesthetic ([3fed562](https://github.com/RouHim/funkstrom/commit/3fed5621e91df9f9226d4334fc6e76cea78f0264))
