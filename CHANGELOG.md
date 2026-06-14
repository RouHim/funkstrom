## [1.4.3](https://github.com/RouHim/funkstrom/compare/1.4.2...1.4.3) (2026-06-14)

## [1.4.2](https://github.com/RouHim/funkstrom/compare/1.4.1...1.4.2) (2026-06-09)


### Bug Fixes

* **audio:** prevent idle FFmpeg thrashing after pause ([106533f](https://github.com/RouHim/funkstrom/commit/106533f461707aeca2bcd6c17aecc8385ee5491f))
* **test:** replace spawn_blocking with try_recv loop in idle-detection test ([b0d6acc](https://github.com/RouHim/funkstrom/commit/b0d6acc381ecba5a0cd0b0d6d070bb25544b18c6))

## [1.4.1](https://github.com/RouHim/funkstrom/compare/1.4.0...1.4.1) (2026-06-06)


### Bug Fixes

* **ci:** copy build.rs into container build context ([f745651](https://github.com/RouHim/funkstrom/commit/f745651760cf05f34687f7976d5cdbdaa86eba61))

# [1.4.0](https://github.com/RouHim/funkstrom/compare/1.3.0...1.4.0) (2026-06-06)


### Features

* **audio:** pause FFmpeg when idle for 60s with no listeners ([91577cb](https://github.com/RouHim/funkstrom/commit/91577cb5aa088dd0cba02283e46785eb0c3551dc))

### Bug Fixes

* **audio:** fix idle FFmpeg thrashing where timeline ticks (~100ms) caused immediate restart after pause, defeating idle CPU savings; wrap idle-pause select in inner loop so only listener connect or sender-drop breaks the pause

# [1.3.0](https://github.com/RouHim/funkstrom/compare/1.2.3...1.3.0) (2026-06-06)


### Features

* show build version in web UI ([87f103c](https://github.com/RouHim/funkstrom/commit/87f103c3dc774e15441765f923f955641f2429ec))
* **audio:** pause FFmpeg transcoding when no listeners for 60s to reduce idle CPU to near zero; resume on first listener connect or timeline change

## [1.2.3](https://github.com/RouHim/funkstrom/compare/1.2.2...1.2.3) (2026-06-05)


### Bug Fixes

* replace bind_address with public_url to fix album art StreamUrl ([1415fb4](https://github.com/RouHim/funkstrom/commit/1415fb4a868069858e4110ee1585862448ae47a3))

## [1.2.3](https://github.com/RouHim/funkstrom/compare/1.2.2...1.2.3) (2026-06-05)


### Bug Fixes

* **server:** replace bind_address config with public_url to fix album art StreamUrl containing unroutable 0.0.0.0; add cover.jpg endpoint test coverage and empty track_path guard

## [1.2.2](https://github.com/RouHim/funkstrom/compare/1.2.1...1.2.2) (2026-06-04)
## [1.2.1](https://github.com/RouHim/funkstrom/compare/1.2.0...1.2.1) (2026-06-04)


### Bug Fixes

* remove initial ICY metadata block that misaligned byte counters ([a61ca40](https://github.com/RouHim/funkstrom/commit/a61ca400c29790c35e2095e5ccfbf4e7fa2922fe))

# [1.2.0](https://github.com/RouHim/funkstrom/compare/1.1.2...1.2.0) (2026-06-04)


### Features

* inject ICY metadata into MP3/AAC streams, pass metadata to FFmpeg for OGG/FLAC ([83d3aae](https://github.com/RouHim/funkstrom/commit/83d3aaedec8b1a061d7b7cb3e72afb9049f5710e))

## [1.1.2](https://github.com/RouHim/funkstrom/compare/1.1.1...1.1.2) (2026-06-04)


### Bug Fixes

* add targeted allow(dead_code) for test-only and unused items ([071107a](https://github.com/RouHim/funkstrom/commit/071107a00fbd58ff775aef97301c6d9c457c84e0))
* P1-P3 code review findings ([5fd74b0](https://github.com/RouHim/funkstrom/commit/5fd74b04ee72dd697aa546fd23ecf1c22ab9c93a))

## [1.1.1](https://github.com/RouHim/funkstrom/compare/1.1.0...1.1.1) (2026-06-04)


### Bug Fixes

* **build:** add retry and content validation to container ffmpeg download ([5613eb6](https://github.com/RouHim/funkstrom/commit/5613eb656c101650f7beb6d845d95d6d38c3ef7c))

# [1.1.0](https://github.com/RouHim/funkstrom/compare/1.0.6...1.1.0) (2026-06-04)


### Bug Fixes

* Add timeout to metadata curl requests in sync tests ([57012fb](https://github.com/RouHim/funkstrom/commit/57012fb539b265c2a047de0188550873d5c79bed))
* **audio:** always pass -ss to ffmpeg to prevent mp3 muxer initial buffering delay ([72ff3ed](https://github.com/RouHim/funkstrom/commit/72ff3ede71f71824d7f2ba29b9e4b7e8f560a9f5))
* **audio:** elevate FFmpeg error logging and fix backoff reset ([ac9bc56](https://github.com/RouHim/funkstrom/commit/ac9bc56ad5a6c2b88cbad65a9ab6d94cd5545165)), closes [#2](https://github.com/RouHim/funkstrom/issues/2) [Hi#Quality](https://github.com/Hi/issues/Quality)
* **audio:** make encoding always-on regardless of listener count ([a410e73](https://github.com/RouHim/funkstrom/commit/a410e7359a142718c3f678d2ab46ea486637c290))
* **audio:** prevent tokio thread starvation in multi-stream encoding ([709c6d9](https://github.com/RouHim/funkstrom/commit/709c6d954379a5c82d076220e7e90086920e6fdd))
* **audio:** remove BufReader from FFmpeg stdout to prevent read starvation ([d6d0bbe](https://github.com/RouHim/funkstrom/commit/d6d0bbe1333ee4729a6aaf016159c627a1137677))
* **ci:** remove prime stream endpoint step to fix e2e listener count tests ([c6780ab](https://github.com/RouHim/funkstrom/commit/c6780ab6e3c01d1bb167004bd5962263a3a461db))
* **playback:** remove is_encoding_active gating from timeline ([7dda055](https://github.com/RouHim/funkstrom/commit/7dda05553f97ec5dffd23d8a4b75d2e23f551b62))
* **ui:** prevent stream switching from restarting playback ([53ff62b](https://github.com/RouHim/funkstrom/commit/53ff62b4a3d17888acb0ea32898d9b4f0dd999ee))


### Features

* **audio:** pause FFmpeg when no listeners and wire through pipeline ([88715c7](https://github.com/RouHim/funkstrom/commit/88715c777c47e6658e34cc94f631788c9e3f07e4))
* **buffer:** add FanoutBuffer with per-listener cursors ([3b8f952](https://github.com/RouHim/funkstrom/commit/3b8f95238c02507b1d5c18fa878f60881c555949))
* **main:** wire timeline-driven architecture ([8cb3295](https://github.com/RouHim/funkstrom/commit/8cb32956662b2fb655a9641c3ae030ea4f667558))
* **scanner:** extract track duration via ffprobe ([cbf18bb](https://github.com/RouHim/funkstrom/commit/cbf18bba74e538d33eeaace1faedf7e947d8e0f8))
* **server:** add per-stream listener counting with RAII guard ([14d3ec0](https://github.com/RouHim/funkstrom/commit/14d3ec0897e3f162863667a9a4fed019a624d0db))
* **timeline:** add PlaybackDirector with virtual clock ([546a155](https://github.com/RouHim/funkstrom/commit/546a1551c6867b48000a0ef860495a82bc2b42a6))

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
