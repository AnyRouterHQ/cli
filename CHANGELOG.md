# Changelog

This file is maintained automatically by [release-please](https://github.com/googleapis/release-please).

GitHub Releases use this file as the release notes (full history through that tag).

## Unreleased

### Features

* **cli:** add `anyr decision` for typed `POST /api/v1/decisions` requests with flag or stdin JSON input. Known Decisions models are refused by chat-agent launchers. ([duyet/anyrouter#3550](https://github.com/duyet/anyrouter/issues/3550))

## [0.1.42](https://github.com/AnyRouterHQ/cli/compare/v0.1.41...v0.1.42) (2026-11-09)


### Features

* **relay:** auto-detect local models at runtime; whitelist via relay models ([9e0e828](https://github.com/AnyRouterHQ/cli/commit/9e0e8289f5ebcdfcc58902a37101171722d11faa))

## [0.1.41](https://github.com/AnyRouterHQ/cli/compare/v0.1.40...v0.1.41) (2026-11-08)


### Features

* **relay:** detect Ollama models locally and advertise them ([982a5e3](https://github.com/AnyRouterHQ/cli/commit/982a5e3540ba0d561c3705bae385f9036a014d35))

## [0.1.40](https://github.com/AnyRouterHQ/cli/compare/v0.1.39...v0.1.40) (2026-11-08)


### Features

* **relay:** serve the Apple system model in-process ([cba112b](https://github.com/AnyRouterHQ/cli/commit/cba112bff66582e4b05cd893f241eab87cd8a046))

## [0.1.39](https://github.com/AnyRouterHQ/cli/compare/v0.1.38...v0.1.39) (2026-11-08)


### Features

* **artifact:** support capabilities.owner_data on publish and visibility ([ffcb4a9](https://github.com/AnyRouterHQ/cli/commit/ffcb4a99b51491fbb4e11ca86df0da0dbd2db137)), closes [#227](https://github.com/AnyRouterHQ/cli/issues/227)
* **launch:** print settled plan as JSON on --json --dry-run ([b83d5f0](https://github.com/AnyRouterHQ/cli/commit/b83d5f070930f6cb031ec3ce5ade36d95da327dd))
* **telemetry:** add once-per-day active-users ping to PostHog ([26cba33](https://github.com/AnyRouterHQ/cli/commit/26cba339699f6fd7fc613ed2c87a07a1b93881f1))
* **tui:** let the command palette fill the terminal height ([fd972c6](https://github.com/AnyRouterHQ/cli/commit/fd972c6f43ce0460c95c9b5fcfdad3206410c484))
* **update:** background auto-upgrade for launcher and agent sessions ([372e9e5](https://github.com/AnyRouterHQ/cli/commit/372e9e5294f27908bcc734d12c2a969be213aae5))


### Bug Fixes

* **artifact:** match per-plan file caps and show quota refusals as-is ([6a43e25](https://github.com/AnyRouterHQ/cli/commit/6a43e25e4b31da189bb6673caa0df51b1ea18abd)), closes [#250](https://github.com/AnyRouterHQ/cli/issues/250)
* **launch:** never persist env/--key API keys to config.yaml; relay: pool sync uses live device id, 409 gets its own message ([98633b2](https://github.com/AnyRouterHQ/cli/commit/98633b233d173d71c4cedc022fbe97df70394b99))
* **launch:** opencode auto registers anyrouter/auto and sets session model ([920adfa](https://github.com/AnyRouterHQ/cli/commit/920adfaaeac8af7432ed9f8eed0d0f6d1054cabc)), closes [#230](https://github.com/AnyRouterHQ/cli/issues/230)
* **pool:** --yolo passes --mode always-allow for interactive pool ([83bc7f3](https://github.com/AnyRouterHQ/cli/commit/83bc7f3612249826742986bf03af77c04baec1fb)), closes [#214](https://github.com/AnyRouterHQ/cli/issues/214)
* **self-update:** [#208](https://github.com/AnyRouterHQ/cli/issues/208) follow-ups and [#194](https://github.com/AnyRouterHQ/cli/issues/194) trash/legacy cleanup ([7c8b87b](https://github.com/AnyRouterHQ/cli/commit/7c8b87be193e10fccd947a5e799de74db32dec69))
* **test:** close [#223](https://github.com/AnyRouterHQ/cli/issues/223) follow-ups ([3c8fcef](https://github.com/AnyRouterHQ/cli/commit/3c8fcefd8a2e70a9b1b41cfd1bfa9858383c95af))
* **test:** pin fixture LF; keep SystemRoot/WINDIR/PATH for Windows detached daily-telemetry child ([ecc8096](https://github.com/AnyRouterHQ/cli/commit/ecc809653444ddd986c054bd28211dfcf4e90bf2))

## [0.1.38](https://github.com/AnyRouterHQ/cli/compare/v0.1.37...v0.1.38) (2026-11-08)


### Features

* **artifact:** raise folder publish limits to match the server ([#228](https://github.com/AnyRouterHQ/cli/issues/228)) ([885c9c1](https://github.com/AnyRouterHQ/cli/commit/885c9c1e6ec61921a60a9d4c27d1e30dd73668a7))
* **keys:** give each coding agent its own key or account ([#252](https://github.com/AnyRouterHQ/cli/issues/252)) ([cbbc659](https://github.com/AnyRouterHQ/cli/commit/cbbc659a821086bb20599bdab75fba68499a85a5))
* **launch:** connect AnyRouter artifact tools in every MCP-capable agent by default ([#226](https://github.com/AnyRouterHQ/cli/issues/226)) ([854e53e](https://github.com/AnyRouterHQ/cli/commit/854e53ee3f8e21a1164e583051b3843317e2b825))
* **launch:** send X-AnyRouter-Title via anyr CLI for app counting ([#282](https://github.com/AnyRouterHQ/cli/issues/282)) ([8503b1f](https://github.com/AnyRouterHQ/cli/commit/8503b1f4c185156f5c5ebc2ec1b0a5198bf62189)), closes [#275](https://github.com/AnyRouterHQ/cli/issues/275)
* **models:** show the model picker as aligned columns ([#242](https://github.com/AnyRouterHQ/cli/issues/242)) ([adaae15](https://github.com/AnyRouterHQ/cli/commit/adaae152bfe1921d8a64e190f1b0e1139df7da13))
* **telemetry:** anonymous install and update ping to PostHog, opt-out ([42e4ffe](https://github.com/AnyRouterHQ/cli/commit/42e4ffefcaafc99313fa0fee6f16b92e6aacfb7b))


### Bug Fixes

* **fx:** cap model_metadata at 256 so fx stops exiting LimitExceeded ([4abaced](https://github.com/AnyRouterHQ/cli/commit/4abaced6d11a67eaaebf13670ff385dd66ffb2ce))
* **fx:** wait for the model catalog before writing the agent's model list ([#260](https://github.com/AnyRouterHQ/cli/issues/260)) ([255508e](https://github.com/AnyRouterHQ/cli/commit/255508e95c2819ab6d9b4d96b75b51fa3e95dcf6))
* **install:** follow the repo transfer to AnyRouterHQ/cli ([3788b10](https://github.com/AnyRouterHQ/cli/commit/3788b10e12bad4547ae9d8da47f5fefa217a486d))
* **keys:** honor --json on keys create ([#281](https://github.com/AnyRouterHQ/cli/issues/281)) ([f084df2](https://github.com/AnyRouterHQ/cli/commit/f084df2840c915146a24c4795946c3451e956928))
* **launch:** codex --last dedupe and grok session_summary model ([#283](https://github.com/AnyRouterHQ/cli/issues/283)) ([319f334](https://github.com/AnyRouterHQ/cli/commit/319f334030ad9f8800c4fe7a0f5033ed4c486892))
* **release:** release notes show only tagged version; move verify-anyr to .agents/skills ([#280](https://github.com/AnyRouterHQ/cli/issues/280)) ([c0de0a4](https://github.com/AnyRouterHQ/cli/commit/c0de0a4688eef2cf901f8f71ae09bfe168d57aa2))
* **telemetry:** stay off under cargo test even for a copied binary ([5e84031](https://github.com/AnyRouterHQ/cli/commit/5e84031934b121ae500e0c9d57a45c353f5efa60))
* **update:** stay silent like no new version when release listing fails ([a031eb6](https://github.com/AnyRouterHQ/cli/commit/a031eb6d4258bf7b76904b977fd99655aa8de60a))


### Performance Improvements

* **models:** open the launcher's model pickers from the disk cache ([#249](https://github.com/AnyRouterHQ/cli/issues/249)) ([6f00a10](https://github.com/AnyRouterHQ/cli/commit/6f00a10ffdd9c5224998649b88671bf191f1362a))

## [0.1.37](https://github.com/anyrouter-dev/cli/compare/v0.1.36...v0.1.37) (2026-10-28)


### Features

* **cli:** errors name a next step and per-command help uses one heading style ([#239](https://github.com/anyrouter-dev/cli/issues/239)) ([3481824](https://github.com/anyrouter-dev/cli/commit/3481824f22b039186d71a02e6bdcc66858177fd0))

## [0.1.36](https://github.com/anyrouter-dev/cli/compare/v0.1.35...v0.1.36) (2026-10-28)


### Bug Fixes

* **fx:** declare every tool-capable chat model so /model can switch ([#236](https://github.com/anyrouter-dev/cli/issues/236)) ([20258fc](https://github.com/anyrouter-dev/cli/commit/20258fc8042f09663d1f62326599440961dc4182))

## [0.1.35](https://github.com/anyrouter-dev/cli/compare/v0.1.34...v0.1.35) (2026-10-27)


### Features

* **keys:** drop the impossible key picker, name device keys, add keys audit ([#232](https://github.com/anyrouter-dev/cli/issues/232)) ([710f40c](https://github.com/anyrouter-dev/cli/commit/710f40c7dd373f3b20b3df896925d791b02df6c5))


### Bug Fixes

* **tui:** plain top-left screens; raw-mode rows back to column 0; rows cut to width ([#234](https://github.com/anyrouter-dev/cli/issues/234)) ([9a9d3f5](https://github.com/anyrouter-dev/cli/commit/9a9d3f55b42aa0582a8ef96184130852b74e7164))

## [0.1.34](https://github.com/anyrouter-dev/cli/compare/v0.1.33...v0.1.34) (2026-10-27)


### Features

* **launch:** add an fx launcher ([#215](https://github.com/anyrouter-dev/cli/issues/215)) ([c25aed7](https://github.com/anyrouter-dev/cli/commit/c25aed7dcb68dc6427846f55df3341e7e9a1c74f))
* **launch:** add qwen, goose, aider, crush and kilo launchers ([#212](https://github.com/anyrouter-dev/cli/issues/212)) ([414dc5a](https://github.com/anyrouter-dev/cli/commit/414dc5a22ef5fb83034fae3c82a72d71b31eb305))
* **launch:** map portable agent flags through one registry ([#213](https://github.com/anyrouter-dev/cli/issues/213)) ([6b49fa6](https://github.com/anyrouter-dev/cli/commit/6b49fa65146fc816d07ecd2164d7e4dd1fe92d28))


### Bug Fixes

* **auth:** show account, credits, plan and key in auth status ([#231](https://github.com/anyrouter-dev/cli/issues/231)) ([e5263c1](https://github.com/anyrouter-dev/cli/commit/e5263c1c7889a6e0af41c946babc158e154723f0))
* **test:** remove the races behind the flaky CI tests ([#222](https://github.com/anyrouter-dev/cli/issues/222)) ([2e8556e](https://github.com/anyrouter-dev/cli/commit/2e8556ea6d1736517ae5c645c7d6cb1f19b097ec))


### Performance Improvements

* **launch:** cache the catalog so a warm launch starts in ~3 ms; anyr config get/set launcher settings ([#217](https://github.com/anyrouter-dev/cli/issues/217)) ([4b3a00c](https://github.com/anyrouter-dev/cli/commit/4b3a00c1b5798d1d9ee06454b3dbf74efcb85071))

## [0.1.33](https://github.com/anyrouter-dev/cli/compare/v0.1.32...v0.1.33) (2026-10-24)


### Features

* **update:** stage updates in the background and apply them on the next run ([#190](https://github.com/anyrouter-dev/cli/issues/190)) ([30e1cee](https://github.com/anyrouter-dev/cli/commit/30e1ceee11d2d4bd667c9a3a87b42027962373a9))


### Bug Fixes

* **launch:** map --skills-off/--tools-off to Pi's --no-skills/--no-tools ([#210](https://github.com/anyrouter-dev/cli/issues/210)) ([8d15f84](https://github.com/anyrouter-dev/cli/commit/8d15f84d742b0d321c051ceb03dc83f7cbc910e0))
* **launch:** no login, install or prompt in scripted runs; pi fetches the catalog once ([#211](https://github.com/anyrouter-dev/cli/issues/211)) ([6f94de2](https://github.com/anyrouter-dev/cli/commit/6f94de26c8282497441168d71bf3b99eca892a48))

## [0.1.32](https://github.com/anyrouter-dev/cli/compare/v0.1.31...v0.1.32) (2026-10-24)


### Features

* **auth:** show name and email in auth status, default model only, add picker ([#201](https://github.com/anyrouter-dev/cli/issues/201)) ([cba42f0](https://github.com/anyrouter-dev/cli/commit/cba42f0b182cc917350d1a10a6960a4d7f524f5f))
* **chat:** footer with model, tokens, timing and cost under the latest reply ([#203](https://github.com/anyrouter-dev/cli/issues/203)) ([7f6336b](https://github.com/anyrouter-dev/cli/commit/7f6336bb0f9bc220279d98a4c7137f880d2373a5))
* **launch:** pass unknown flags to the agent; --yolo maps per agent ([#200](https://github.com/anyrouter-dev/cli/issues/200)) ([d36b0ab](https://github.com/anyrouter-dev/cli/commit/d36b0ab08b8a1d1fe5ed0fd908c60f433e88960d))


### Bug Fixes

* **relay:** derive websocket and pool API URLs from the base URL ([#207](https://github.com/anyrouter-dev/cli/issues/207)) ([b4fe1b5](https://github.com/anyrouter-dev/cli/commit/b4fe1b52ead054e900ef4dc1d5a3b2009ab8c4ee))
* **relay:** detect half-open sockets, back off with jitter, bound memory ([#204](https://github.com/anyrouter-dev/cli/issues/204)) ([5736d45](https://github.com/anyrouter-dev/cli/commit/5736d45e0468b95e75abc77cee9b2b1aaee00caf))

## [0.1.31](https://github.com/anyrouter-dev/cli/compare/v0.1.30...v0.1.31) (2026-10-24)


### Features

* **agent:** --json and JSON errors on every command, one exit-code table ([#193](https://github.com/anyrouter-dev/cli/issues/193)) ([18dfbfe](https://github.com/anyrouter-dev/cli/commit/18dfbfe8b16884ac79dfb6b3904bbc831a4c0067))
* **help:** short --help with the everyday commands; help --all from the registry ([#195](https://github.com/anyrouter-dev/cli/issues/195)) ([24fdb2a](https://github.com/anyrouter-dev/cli/commit/24fdb2a4a94c1c1ab99f772b68c7269a45725948))
* **mcp:** connect the AnyRouter MCP gateway when launching an agent ([#184](https://github.com/anyrouter-dev/cli/issues/184)) ([327608e](https://github.com/anyrouter-dev/cli/commit/327608ef81f435d12b7d77710984810111230bb4))
* **relay:** say how to serve the Decider when it is missing, stopped or off-Mac ([#199](https://github.com/anyrouter-dev/cli/issues/199)) ([596f721](https://github.com/anyrouter-dev/cli/commit/596f721487e56b420978ff5d3ddbcdb8c9160872))


### Bug Fixes

* **cli:** mask API keys in error output ([#197](https://github.com/anyrouter-dev/cli/issues/197)) ([55ed01f](https://github.com/anyrouter-dev/cli/commit/55ed01f6374fdc3751b3ced79512561728775a9d))
* **relay:** bind each machine to its own device and send pool in hello ([#187](https://github.com/anyrouter-dev/cli/issues/187)) ([a1531dd](https://github.com/anyrouter-dev/cli/commit/a1531dd426b77da88f4948630156c9ce0152d4f9))


### Performance Improvements

* **build:** fat LTO and opt-level z, -20% binary ([#191](https://github.com/anyrouter-dev/cli/issues/191)) ([7728831](https://github.com/anyrouter-dev/cli/commit/77288315efbeb507dc797dee49e044bff0275bb7))

## [0.1.30](https://github.com/anyrouter-dev/cli/compare/v0.1.29...v0.1.30) (2026-10-24)


### Features

* **relay:** name the device after this Mac, keep it, add relay rename ([#183](https://github.com/anyrouter-dev/cli/issues/183)) ([6c179a6](https://github.com/anyrouter-dev/cli/commit/6c179a6c548df5f0cf32729d563bee5d1a6b0f0d))
* **relay:** start fm serve and the Decider app from relay start ([#185](https://github.com/anyrouter-dev/cli/issues/185)) ([85e4441](https://github.com/anyrouter-dev/cli/commit/85e4441d04bf27563bb81dff8ce9e795d11ea6ed))


### Bug Fixes

* **grok:** send the AnyRouter key even when a grok login session exists ([#186](https://github.com/anyrouter-dev/cli/issues/186)) ([63d4aae](https://github.com/anyrouter-dev/cli/commit/63d4aae391ba10f109ef4c6203f890ea4e4d0b7e))
* **opencode:** run OpenCode 2.x standalone so the AnyRouter provider applies ([#181](https://github.com/anyrouter-dev/cli/issues/181)) ([2f9939c](https://github.com/anyrouter-dev/cli/commit/2f9939ceda69b67f14c04268c58e45ea16009f33))
* **pi:** read the AnyRouter key through a command so current Pi stops sending the env name ([#173](https://github.com/anyrouter-dev/cli/issues/173)) ([97e8cf7](https://github.com/anyrouter-dev/cli/commit/97e8cf764aaa8794ac9532ecb2c61863da5c76fb))
* **presets:** launch with --preset on an inference key instead of demanding a management key ([#178](https://github.com/anyrouter-dev/cli/issues/178)) ([5d3479d](https://github.com/anyrouter-dev/cli/commit/5d3479d7925665c3eee85aeaf0eac48fd944d11c))

## [0.1.29](https://github.com/anyrouter-dev/cli/compare/v0.1.28...v0.1.29) (2026-10-24)


### Features

* **artifact:** --show-author / --no-show-author on publish and visibility ([#166](https://github.com/anyrouter-dev/cli/issues/166)) ([e80babf](https://github.com/anyrouter-dev/cli/commit/e80babfcf6c604cfd6e9aa514a62199ec3f5b9f5))
* **cli:** add anyr generation &lt;id&gt; (alias gen) ([#162](https://github.com/anyrouter-dev/cli/issues/162)) ([d29a606](https://github.com/anyrouter-dev/cli/commit/d29a6061a70016b8af5bc0ee75663a7fc4408fb8))
* **cli:** add anyr pool stats|providers|free|relay ([#163](https://github.com/anyrouter-dev/cli/issues/163)) ([473b9d8](https://github.com/anyrouter-dev/cli/commit/473b9d87e3bb6913d71c4a27b5596f4077595ccf))
* **delegate:** hand a task to another coding agent ([#172](https://github.com/anyrouter-dev/cli/issues/172)) ([f868961](https://github.com/anyrouter-dev/cli/commit/f868961b86ca3762ac02298a4f27f92bad298e8f))
* **eval:** add anyr eval to compare one prompt across models ([#169](https://github.com/anyrouter-dev/cli/issues/169)) ([09371ac](https://github.com/anyrouter-dev/cli/commit/09371ace88e2e3d69f93377cb2eb9aa46f0df6b6))
* **mcp:** add anyr mcp url, add and connections ([#167](https://github.com/anyrouter-dev/cli/issues/167)) ([aa24e4e](https://github.com/anyrouter-dev/cli/commit/aa24e4eaa169c7c8d33cc369ce46794144edb7ef))
* **presets:** add anyr presets and --preset on launch ([#165](https://github.com/anyrouter-dev/cli/issues/165)) ([cb4ba73](https://github.com/anyrouter-dev/cli/commit/cb4ba730da4e4cd2a4ca872c090cefa33ba1342f))
* **publish:** upload an .html or .svg page with its light build ([#168](https://github.com/anyrouter-dev/cli/issues/168)) ([9b15deb](https://github.com/anyrouter-dev/cli/commit/9b15deb4c30ef57abe807c3715674bfd5fe77ad5))
* **relay:** add devices list/rename/pool/rm and models --devices ([#164](https://github.com/anyrouter-dev/cli/issues/164)) ([9eca1a8](https://github.com/anyrouter-dev/cli/commit/9eca1a8e79cef607502309350bb600b5f12ef6f7))
* **task:** plan with one model, implement with another ([#171](https://github.com/anyrouter-dev/cli/issues/171)) ([549944f](https://github.com/anyrouter-dev/cli/commit/549944fc42615b2c9bcd128bdef7add12cefedaf))


### Bug Fixes

* **artifact:** page through ls and fail clearly when a file changes mid-publish ([#170](https://github.com/anyrouter-dev/cli/issues/170)) ([61e8ad6](https://github.com/anyrouter-dev/cli/commit/61e8ad621ae4d60e9b110f19e8e541b9b678045a))
* **artifact:** say a private link is not shareable and how to share it ([#161](https://github.com/anyrouter-dev/cli/issues/161)) ([cfd9065](https://github.com/anyrouter-dev/cli/commit/cfd9065a522745a1d33f9b1c3f50ce4d7400f06e))
* **ci:** scope pull-requests write to the report job ([#158](https://github.com/anyrouter-dev/cli/issues/158)) ([8e7e90b](https://github.com/anyrouter-dev/cli/commit/8e7e90bc1fb25870d037ed62486cdf7bfcc67c43)), closes [#157](https://github.com/anyrouter-dev/cli/issues/157)

## [0.1.28](https://github.com/anyrouter-dev/cli/compare/v0.1.27...v0.1.28) (2026-10-23)


### Bug Fixes

* **cli:** exit quietly when stdout is a closed pipe ([#155](https://github.com/anyrouter-dev/cli/issues/155)) ([fa7701a](https://github.com/anyrouter-dev/cli/commit/fa7701a16144c50acc3609d420855d4bcea03795))

## [0.1.27](https://github.com/anyrouter-dev/cli/compare/v0.1.26...v0.1.27) (2026-10-23)


### Features

* **byok:** add rm, test and providers ([#151](https://github.com/anyrouter-dev/cli/issues/151)) ([e1c5d83](https://github.com/anyrouter-dev/cli/commit/e1c5d832e867b80d3716973cc77121655fa5e34e))
* **embed:** add anyr embed ([#149](https://github.com/anyrouter-dev/cli/issues/149)) ([a5c9453](https://github.com/anyrouter-dev/cli/commit/a5c9453440e317a32f6bcef84a6d39abde30be4d))
* **keys:** scoping flags on create, plus keys show and update ([#153](https://github.com/anyrouter-dev/cli/issues/153)) ([b910aa6](https://github.com/anyrouter-dev/cli/commit/b910aa6187e30ba7d3f5c83cd66b8c6d55ec0820))
* **models:** add --free/--type/--owner/--search/--sort filters and models show ([#152](https://github.com/anyrouter-dev/cli/issues/152)) ([878c35c](https://github.com/anyrouter-dev/cli/commit/878c35cacd162e1964f79034778ef72e416db627))

## [0.1.26](https://github.com/anyrouter-dev/cli/compare/v0.1.25...v0.1.26) (2026-10-23)


### Features

* **logs:** implement anyr logs ([#146](https://github.com/anyrouter-dev/cli/issues/146)) ([4bc753e](https://github.com/anyrouter-dev/cli/commit/4bc753e6559415f8d7bff56de5a19ae0e0fdcd73))
* **transactions:** implement anyr transactions ([#147](https://github.com/anyrouter-dev/cli/issues/147)) ([57dd7e2](https://github.com/anyrouter-dev/cli/commit/57dd7e26f25ef53a7678b57f4c5531d62f5a10d9))

## [0.1.25](https://github.com/anyrouter-dev/cli/compare/v0.1.24...v0.1.25) (2026-10-23)


### Features

* **relay:** simple, controllable relay UX (status, detach, stop, logs, models, pool, doctor) ([#140](https://github.com/anyrouter-dev/cli/issues/140)) ([83665c9](https://github.com/anyrouter-dev/cli/commit/83665c9ae67a009941f0e24669340bbcd3bc163a))

## [0.1.24](https://github.com/anyrouter-dev/cli/compare/v0.1.23...v0.1.24) (2026-10-23)


### Features

* **chat:** stream a reply from /v1/chat/completions ([#144](https://github.com/anyrouter-dev/cli/issues/144)) ([5c9f3ac](https://github.com/anyrouter-dev/cli/commit/5c9f3ac8f32a93d793a759a5db88245543ceaa89)), closes [#91](https://github.com/anyrouter-dev/cli/issues/91)
* **models:** add a type column to piped anyr models ([#122](https://github.com/anyrouter-dev/cli/issues/122)) ([e748722](https://github.com/anyrouter-dev/cli/commit/e7487225447de8c9e4d03b131290e51480ff1b48)), closes [#93](https://github.com/anyrouter-dev/cli/issues/93)


### Bug Fixes

* **artifact:** review follow-ups from [#110](https://github.com/anyrouter-dev/cli/issues/110) ([#141](https://github.com/anyrouter-dev/cli/issues/141)) ([0c8fea8](https://github.com/anyrouter-dev/cli/commit/0c8fea8d2764f39cd70114cc048087a61ed8212a))
* **cli:** name the missing scope on restricted keys and unify error output ([#126](https://github.com/anyrouter-dev/cli/issues/126)) ([c2b0c71](https://github.com/anyrouter-dev/cli/commit/c2b0c714a89f1b9b14a6537337d55c92743a804d))
* **codex:** pass model_context_window for a concrete model ([#139](https://github.com/anyrouter-dev/cli/issues/139)) ([f0a9dd5](https://github.com/anyrouter-dev/cli/commit/f0a9dd50dc0451c1ecd5c83cfd13d342a48f2a30)), closes [#101](https://github.com/anyrouter-dev/cli/issues/101)

## [0.1.23](https://github.com/anyrouter-dev/cli/compare/v0.1.22...v0.1.23) (2026-10-23)


### Features

* **relay:** serve on-device models by default, pool on unless --no-pool ([#120](https://github.com/anyrouter-dev/cli/issues/120)) ([1274dd0](https://github.com/anyrouter-dev/cli/commit/1274dd0b7b072c9b00fffe0657d8db7f461f06a2))

## [0.1.22](https://github.com/anyrouter-dev/cli/compare/v0.1.21...v0.1.22) (2026-10-23)


### Features

* **apps:** say whether a visibility change waits for review or is listed ([#118](https://github.com/anyrouter-dev/cli/issues/118)) ([4d538cd](https://github.com/anyrouter-dev/cli/commit/4d538cdfe2f07eee0a1f2acffc6ffc13e13f8979))
* **artifact:** add anyr artifact publish, ls, rm, visibility ([#110](https://github.com/anyrouter-dev/cli/issues/110)) ([c6fb883](https://github.com/anyrouter-dev/cli/commit/c6fb8838db662df70b44d8819a4111ad220a6cb0))
* **publish:** generic anyr publish and apps, api --data @file/- ([#108](https://github.com/anyrouter-dev/cli/issues/108)) ([3662f3f](https://github.com/anyrouter-dev/cli/commit/3662f3f0a1663ececa4269d1ae909eb5b0f4fd75))
* **relay:** serve several local endpoints and route by model ([#117](https://github.com/anyrouter-dev/cli/issues/117)) ([27769e4](https://github.com/anyrouter-dev/cli/commit/27769e43a14ba9a13f2113de289410b15d6b299b))


### Bug Fixes

* **artifact:** send unlisted, keep link as an alias ([#119](https://github.com/anyrouter-dev/cli/issues/119)) ([1921961](https://github.com/anyrouter-dev/cli/commit/1921961ed0de033131153f07ae595483ee1ac63e))
* **decision:** show the server error and a working help example ([#112](https://github.com/anyrouter-dev/cli/issues/112)) ([1dcc4b5](https://github.com/anyrouter-dev/cli/commit/1dcc4b5d9a25da645490fafc4f6ced6def152340))
* **relay:** treat a Windows read timeout as a poll, not a disconnect ([#113](https://github.com/anyrouter-dev/cli/issues/113)) ([3bf61f0](https://github.com/anyrouter-dev/cli/commit/3bf61f0403ef3ce09e05a25941988cfdbf86c8f2))
* **upgrade:** let the beta channel take a newer stable release ([#111](https://github.com/anyrouter-dev/cli/issues/111)) ([7332487](https://github.com/anyrouter-dev/cli/commit/733248744527d79fb049df2e69c9ba2a2a2663df))

## [0.1.21](https://github.com/anyrouter-dev/cli/compare/v0.1.20...v0.1.21) (2026-10-21)


### Bug Fixes

* **claude:** declare the model context window to Claude Code ([#102](https://github.com/anyrouter-dev/cli/issues/102)) ([6e969b2](https://github.com/anyrouter-dev/cli/commit/6e969b23fc30b424f5b679ffe148a875ef76abff))
* **cli:** default pi and claude aliases to anyrouter/free ([#107](https://github.com/anyrouter-dev/cli/issues/107)) ([e52c171](https://github.com/anyrouter-dev/cli/commit/e52c171072ad3b8e78d3aa430e2f2a784f994129))
* **cli:** review round 6 — help text, usage errors ([#96](https://github.com/anyrouter-dev/cli/issues/96)) ([3b9909b](https://github.com/anyrouter-dev/cli/commit/3b9909b17dad21e4be0e57190a54e178985f8946))
* **pi:** declare each model's context window in Pi models.json ([#106](https://github.com/anyrouter-dev/cli/issues/106)) ([3302bc6](https://github.com/anyrouter-dev/cli/commit/3302bc6e078b8e0e83bac55651f2a8dff61542c9))
* **relay:** connect with the sk-ar key directly and stop on 401/403 ([#97](https://github.com/anyrouter-dev/cli/issues/97)) ([283516b](https://github.com/anyrouter-dev/cli/commit/283516be34791a3d9d0b6488269d56e701b6b853))

## [0.1.20](https://github.com/anyrouter-dev/cli/compare/v0.1.19...v0.1.20) (2026-10-02)


### Bug Fixes

* **cli:** round 5 — env key, flag consistency, error bodies, README ([#87](https://github.com/anyrouter-dev/cli/issues/87)) ([1fcfa27](https://github.com/anyrouter-dev/cli/commit/1fcfa27742fb01fdc1600e3e9c49e5026fc8333d))

## [0.1.19](https://github.com/anyrouter-dev/cli/compare/v0.1.18...v0.1.19) (2026-10-02)


### Bug Fixes

* **cli:** drop ar symlink, relay connect timeout, --key - everywhere ([#85](https://github.com/anyrouter-dev/cli/issues/85)) ([908d2f7](https://github.com/anyrouter-dev/cli/commit/908d2f7dd6593c7cce5f7c80bdfd16931b29fcc7))

## [0.1.18](https://github.com/anyrouter-dev/cli/compare/v0.1.17...v0.1.18) (2026-10-02)


### Bug Fixes

* **cli:** round 3 audit fixes ([#83](https://github.com/anyrouter-dev/cli/issues/83)) ([b9cf287](https://github.com/anyrouter-dev/cli/commit/b9cf28746b991ba2495b5a0ebe27b36a1ffb6007))

## [0.1.17](https://github.com/anyrouter-dev/cli/compare/v0.1.16...v0.1.17) (2026-10-02)


### Bug Fixes

* **cli:** consistent usage exit codes, help topics, private state files ([#80](https://github.com/anyrouter-dev/cli/issues/80)) ([20296f1](https://github.com/anyrouter-dev/cli/commit/20296f12b45514285bf31a45fe6e5a34180a85cd))
* **release:** keep Cargo.lock version in sync with release-please ([#79](https://github.com/anyrouter-dev/cli/issues/79)) ([7c2dd59](https://github.com/anyrouter-dev/cli/commit/7c2dd59d9b815616f54ce4cb4f42ef1a733efa8c))

## [0.1.16](https://github.com/anyrouter-dev/cli/compare/v0.1.15...v0.1.16) (2026-10-02)


### Features

* **cli:** add native Decisions API command ([#75](https://github.com/anyrouter-dev/cli/issues/75)) ([1d68eac](https://github.com/anyrouter-dev/cli/commit/1d68eac955dc91f162626d1b79d5c9095fe74e94))
* **cli:** anyr api dashboard commands and shell completion ([#77](https://github.com/anyrouter-dev/cli/issues/77)) ([00ccc7a](https://github.com/anyrouter-dev/cli/commit/00ccc7a7dda8d3548bfce51ba06e61624ab5c561))
* **cli:** byok --donate with source badges, upgrade fixes, model alias ([#78](https://github.com/anyrouter-dev/cli/issues/78)) ([059a34b](https://github.com/anyrouter-dev/cli/commit/059a34b783c06caef13786abca933f3fbcb32c4c))


### Bug Fixes

* **ci:** always upload checksums.txt; unglue update spinner warnings ([#74](https://github.com/anyrouter-dev/cli/issues/74)) ([3038fb7](https://github.com/anyrouter-dev/cli/commit/3038fb720cdf6e97f1568edb5fff2d205a84bdad))

## [0.1.15](https://github.com/anyrouter-dev/cli/compare/v0.1.14...v0.1.15) (2026-09-16)


### Bug Fixes

* **cli:** keep Claude HUD floors on virtual presets end-to-end (ANTHROPIC_MODEL, extra-body min_context, compact window, no catalog remap)
* **cli:** bare `anyr update` keeps the config channel; only `--beta`/`--stable` persist a switch
* **cli:** skip a broken latest GitHub release (checksum / missing asset) and install the next good build on that channel
* **cli:** print update warnings on a new line so they are not glued to the spinner
* **ci:** upload checksums.txt even when one platform build fails; retry Windows release-not-found uploads


## [0.1.14](https://github.com/anyrouter-dev/cli/compare/v0.1.13...v0.1.14) (2026-09-15)


### Features

* **cli:** hint Ctrl+G restart after auto-upgrade ([#71](https://github.com/anyrouter-dev/cli/issues/71)) ([0cbd8b1](https://github.com/anyrouter-dev/cli/commit/0cbd8b1fc52e6def51adf98c1c501b428d046b5a))


### Bug Fixes

* **cli:** keep [1m]/[500k] on Claude virtual ANTHROPIC_MODEL ([#69](https://github.com/anyrouter-dev/cli/issues/69)) ([9507bef](https://github.com/anyrouter-dev/cli/commit/9507bef908dc96ef849e49d95451f83f0cda39de))

## [0.1.13](https://github.com/anyrouter-dev/cli/compare/v0.1.12...v0.1.13) (2026-09-15)


### Features

* **cli:** peel [1m]/[500k] on all anyrouter virtual presets ([#67](https://github.com/anyrouter-dev/cli/issues/67)) ([97fba31](https://github.com/anyrouter-dev/cli/commit/97fba3144c178ce9e27a5e12118b7ba29bd5a3dc))

## [0.1.12](https://github.com/anyrouter-dev/cli/compare/v0.1.11...v0.1.12) (2026-09-15)


### Features

* --model anyrouter/auto[1m] and [500k] min-context floors ([#62](https://github.com/anyrouter-dev/cli/issues/62)) ([524230e](https://github.com/anyrouter-dev/cli/commit/524230ed1a56f3d0aba08b4d7db70586219a81c6))
* **cli:** animate anyr update with from→to and channel ([#41](https://github.com/anyrouter-dev/cli/issues/41)) ([18be07f](https://github.com/anyrouter-dev/cli/commit/18be07f9c78db5bb90f8e0eccca84f2f189c8299))
* **cli:** Compact HUD launcher ([#51](https://github.com/anyrouter-dev/cli/issues/51)) ([3dd0d12](https://github.com/anyrouter-dev/cli/commit/3dd0d123aa8e6876e1e32e057e6214dd10a5c498))
* **cli:** compact HUD picker, examples-first help, status config ([#53](https://github.com/anyrouter-dev/cli/issues/53)) ([7c1bbd9](https://github.com/anyrouter-dev/cli/commit/7c1bbd9979c18439e3fe15ede3a2fd99d8d28292))
* **cli:** default omitted --model to anyrouter/auto ([#61](https://github.com/anyrouter-dev/cli/issues/61)) ([b6ca882](https://github.com/anyrouter-dev/cli/commit/b6ca88261570a36fe16f67988695311e52c13211))
* **cli:** launch Claude right after install ([#54](https://github.com/anyrouter-dev/cli/issues/54)) ([b37c9c5](https://github.com/anyrouter-dev/cli/commit/b37c9c5cee920cfaf6caf07f4f5c2c3bab5be782))
* **cli:** spotlight chrome — examples-first help, brand orange, models table ([#50](https://github.com/anyrouter-dev/cli/issues/50)) ([7094017](https://github.com/anyrouter-dev/cli/commit/7094017c0e89545e9cfcae680982ab005886e783))
* **relay:** anyr relay start/pair — serve local models to the cloud ([0ee4699](https://github.com/anyrouter-dev/cli/commit/0ee4699572080fddfc49929322e99df73273bc48))
* **security:** read pasted API keys with terminal echo off ([#31](https://github.com/anyrouter-dev/cli/issues/31)) ([d6359c8](https://github.com/anyrouter-dev/cli/commit/d6359c8b14dbf3303b26940895591228e67a61bb)), closes [#21](https://github.com/anyrouter-dev/cli/issues/21)
* **security:** verify sha256 of downloaded release assets before replace ([#30](https://github.com/anyrouter-dev/cli/issues/30)) ([211c7bc](https://github.com/anyrouter-dev/cli/commit/211c7bcbcf71071d483aaa6790dfaba446e206a5)), closes [#20](https://github.com/anyrouter-dev/cli/issues/20)
* **tui:** AR-mark palette card, agent rows, settings polish ([866d7b0](https://github.com/anyrouter-dev/cli/commit/866d7b061c6214d1950a01df76c8ecc8fecf0eb1))
* **tui:** per-agent routing filters (exacto, tools, 1M ctx) ([#44](https://github.com/anyrouter-dev/cli/issues/44)) ([5ee9ce3](https://github.com/anyrouter-dev/cli/commit/5ee9ce396ffbd5875893fcf0fd042b508af56c7c))


### Bug Fixes

* **auth:** honor expires_in and bound transient network retries in device login ([#28](https://github.com/anyrouter-dev/cli/issues/28)) ([689b54f](https://github.com/anyrouter-dev/cli/commit/689b54fa98a52dd4c8e5dc7a0d742d167fa7bb96)), closes [#18](https://github.com/anyrouter-dev/cli/issues/18)
* **ci:** build darwin-x86_64 in the main matrix so beta installs work on Intel Macs ([#26](https://github.com/anyrouter-dev/cli/issues/26)) ([db13555](https://github.com/anyrouter-dev/cli/commit/db1355506269538e959278c9cb2193717424743c)), closes [#16](https://github.com/anyrouter-dev/cli/issues/16)
* **cli:** avoid GitHub Releases API 403 and empty /latest 404 ([#33](https://github.com/anyrouter-dev/cli/issues/33)) ([ca6e78e](https://github.com/anyrouter-dev/cli/commit/ca6e78eff4daa8b00191e92012fdc039545495d6)), closes [#32](https://github.com/anyrouter-dev/cli/issues/32)
* **cli:** fail loud on empty release assets; reject dummy test keys ([ba8b62f](https://github.com/anyrouter-dev/cli/commit/ba8b62f720f22ecbff1adba0b2a5c3a4d8f0a931))
* **cli:** fresh path keeps auto -&gt; anyrouter/auto for first-turn failover ([#58](https://github.com/anyrouter-dev/cli/issues/58)) ([#59](https://github.com/anyrouter-dev/cli/issues/59)) ([3f020ff](https://github.com/anyrouter-dev/cli/commit/3f020fffb8cd1a55b4213b6a2d9a0688897d3080))
* **cli:** keep anyrouter/auto[1m] as auto plus min_context ([#66](https://github.com/anyrouter-dev/cli/issues/66)) ([e446fa7](https://github.com/anyrouter-dev/cli/commit/e446fa797ce2dbdebee8b9ab6db26862e8cb150c)), closes [#65](https://github.com/anyrouter-dev/cli/issues/65)
* **cli:** keep TUI keys, tool yaml, and relay from dropping state ([#52](https://github.com/anyrouter-dev/cli/issues/52)) ([30547a2](https://github.com/anyrouter-dev/cli/commit/30547a246f2d7de8fe5b8bd3605d6fa4fdfafade))
* **cli:** stop appending [1m] to Claude catalog model ids ([#60](https://github.com/anyrouter-dev/cli/issues/60)) ([81a5473](https://github.com/anyrouter-dev/cli/commit/81a547314e644b1d84a13277f068887866150d97))
* **cli:** stop the login re-reveal loop; persist profile across relogin ([358bcca](https://github.com/anyrouter-dev/cli/commit/358bcca0534653141bccbb01166721a57bf7bbb3))
* **menu:** show HUD status line before actions when menu runs non-interactively ([#57](https://github.com/anyrouter-dev/cli/issues/57)) ([d54ffbe](https://github.com/anyrouter-dev/cli/commit/d54ffbeaa60639f5de6645955712de79b385f1d9))
* **pi:** write full catalog into models.json for /model picker ([b4941e2](https://github.com/anyrouter-dev/cli/commit/b4941e2945aad566636087e2a6cb22fc9c213b87))
* **relay:** never panic on multibyte request ids when naming worker threads ([#27](https://github.com/anyrouter-dev/cli/issues/27)) ([77adf3a](https://github.com/anyrouter-dev/cli/commit/77adf3aefd1b0a1c857035c0400d6032777219dc)), closes [#17](https://github.com/anyrouter-dev/cli/issues/17)
* **security:** write config atomically with 0700 dir / 0600 file ([#29](https://github.com/anyrouter-dev/cli/issues/29)) ([9ca7ac7](https://github.com/anyrouter-dev/cli/commit/9ca7ac7ea2a535f9059b42426b276d0e07f296dc)), closes [#19](https://github.com/anyrouter-dev/cli/issues/19)
* **tui:** bind model/account/key per coding agent ([#39](https://github.com/anyrouter-dev/cli/issues/39)) ([97d3370](https://github.com/anyrouter-dev/cli/commit/97d3370c144e3f53603f2b05c21e003577e8a8d5))
* **tui:** pin anyrouter/auto in the inline model picker ([#40](https://github.com/anyrouter-dev/cli/issues/40)) ([59b1f6a](https://github.com/anyrouter-dev/cli/commit/59b1f6aae4346db8d459de471fdbdde926457917))

## [0.1.11](https://github.com/anyrouter-dev/cli/compare/v0.1.10...v0.1.11) (2026-08-24)


### Features

* **cli:** ar update --beta|--stable switches channel and updates ([4b3db1b](https://github.com/anyrouter-dev/cli/commit/4b3db1b3ecf903805b9a52f12020a366da6fc6e6))
* **cli:** ar update --beta|--stable switches channel and updates ([dcf1b60](https://github.com/anyrouter-dev/cli/commit/dcf1b6035966b1c1f9eb7c7e0a554e51ce3e601e))
* **cli:** embed build time, shown in local timezone ([2fd669d](https://github.com/anyrouter-dev/cli/commit/2fd669dd8af37123a994932ce04f71024dd5e9d7))
* **cli:** looping launcher with config, login, and agent pick ([3b901c1](https://github.com/anyrouter-dev/cli/commit/3b901c1bc38782ee99825349020c71e9b9e5bc25))
* **cli:** model pin collapses alias slots; fable slot; build time ([256e0cb](https://github.com/anyrouter-dev/cli/commit/256e0cbcf5c84aaaf8eea419dbdbd155d5c67ec7))
* **cli:** pin model collapses claude alias slots; add fable slot ([96ef51a](https://github.com/anyrouter-dev/cli/commit/96ef51ae14ebe4471cfcf3539018bc21a3b2bc07))
* **cli:** Ratatui interactive TUI as the default launcher ([ee93b54](https://github.com/anyrouter-dev/cli/commit/ee93b54f0789dc9140db60dc305254c993db126c))
* **cli:** Ratatui launcher TUI as default ([aa7d4fb](https://github.com/anyrouter-dev/cli/commit/aa7d4fb3f22773840a394df1c912e5d58f19693d))
* **cli:** two-pane launcher with icons and cached credits ([26f9980](https://github.com/anyrouter-dev/cli/commit/26f9980649c2b4895b45f5844fd4cacabbc0167d))
* **tui:** centered dialog welcome launcher ([61a22ee](https://github.com/anyrouter-dev/cli/commit/61a22ee98e6d8130dce27f153497ebf0349c1ba5))
* **tui:** command-palette launcher with inline fallback ([ba25454](https://github.com/anyrouter-dev/cli/commit/ba25454fd665aed782c39322b3df81fd8a6a0f8d))
* **tui:** interactive config screen with identity and model memory ([#8](https://github.com/anyrouter-dev/cli/issues/8)) ([8a343d6](https://github.com/anyrouter-dev/cli/commit/8a343d67f5e3344f5b5102ec1f371b7d7de8013b))
* **tui:** show installed agents and per-agent settings tabs ([#10](https://github.com/anyrouter-dev/cli/issues/10)) ([8c7554c](https://github.com/anyrouter-dev/cli/commit/8c7554c84ddb7b23d9166f445e2921e3cedf0675))


### Bug Fixes

* **ci:** unblock stable release binaries ([99ba2bc](https://github.com/anyrouter-dev/cli/commit/99ba2bcb09a985249bf191e706e73d9a3e35af2c))
* **cli:** gate days_from_civil test to unix ([276098d](https://github.com/anyrouter-dev/cli/commit/276098d459d3d5576b7a1533738b96bab2312ba9))
* **cli:** label update current, latest, and status ([#14](https://github.com/anyrouter-dev/cli/issues/14)) ([629336a](https://github.com/anyrouter-dev/cli/commit/629336a041e1ab38e89e4ac26b4de6339dd7d5cf))
* **models:** catalog ids, Claude [1m] suffix, auto = most used ([#12](https://github.com/anyrouter-dev/cli/issues/12)) ([7fc97ba](https://github.com/anyrouter-dev/cli/commit/7fc97ba96748f02568a5cb20d23164166f5093f7))
* **pi:** pass catalog model ids and strip ANSI from --model ([#11](https://github.com/anyrouter-dev/cli/issues/11)) ([aad7641](https://github.com/anyrouter-dev/cli/commit/aad7641d4ac92fcfb4ba50aaf940c8488429fd37))
* **tui:** pick keys, accounts, and models from the CLI ([#9](https://github.com/anyrouter-dev/cli/issues/9)) ([ad95180](https://github.com/anyrouter-dev/cli/commit/ad951805eecb7ee3538c5c1844af8a8099194633))
* **wasm:** gate launcher palette entries behind native feature ([ddc7887](https://github.com/anyrouter-dev/cli/commit/ddc788721473a7a5074de457fb7b1dcada22a036))


### Performance Improvements

* **tui:** open the launcher without blocking on network or which ([#13](https://github.com/anyrouter-dev/cli/issues/13)) ([58fdb8b](https://github.com/anyrouter-dev/cli/commit/58fdb8b16dd8d6b532b57250db89dd00afdcd57e))

## [0.1.10](https://github.com/anyrouter-dev/cli/compare/v0.1.9...v0.1.10) (2026-08-21)


### Features

* **ci:** build every platform, bench size/startup, ship wasm demo ([532a1e0](https://github.com/anyrouter-dev/cli/commit/532a1e08f31a8bcd2567cc5db2269a30ff9f27e4))
* **cli:** auto-update in the background by default ([f3d5edd](https://github.com/anyrouter-dev/cli/commit/f3d5edd43c3185153af4ac88647670ea7f1faff2))
* **cli:** device login, key/model switch, agent install, color TUI ([cc4cccd](https://github.com/anyrouter-dev/cli/commit/cc4cccd144038a0454ba377648e18868e5ddc4c8))
* **cli:** draw a small AR mark in the terminal ([b7a313b](https://github.com/anyrouter-dev/cli/commit/b7a313b68e0bc4154f957351e59836fc7dbd23e7))
* **cli:** draw the AR mark on more terminals ([75e2a79](https://github.com/anyrouter-dev/cli/commit/75e2a79c83845cfaec0c8df446986a6d1e9be5ea))
* **cli:** gh-style auth command group ([47563d6](https://github.com/anyrouter-dev/cli/commit/47563d6e69b7d4b84067c151d329d7c31e361436))
* **cli:** interactive TUI for ar config ([b2c7d1a](https://github.com/anyrouter-dev/cli/commit/b2c7d1a5574fe7951d37ca9073e4271196215f9a))
* **cli:** native anyr 0.1.x with GitHub Releases install ([836f73f](https://github.com/anyrouter-dev/cli/commit/836f73f48c3a8d48d06c1c4ea49fef9f25e83694))
* **cli:** publish a beta from every main push ([ff71b81](https://github.com/anyrouter-dev/cli/commit/ff71b812b056e97eefd36914406db6c5c786752b))
* **cli:** spawn pi alongside claude, codex, and opencode ([56f28d0](https://github.com/anyrouter-dev/cli/commit/56f28d045c8aaa1ededc01e1155927134ffe0474))


### Bug Fixes

* **ci:** resolve release bench binary by absolute path ([4291216](https://github.com/anyrouter-dev/cli/commit/429121641f56e004f7c7fbba596ffbe7eccc2b81))
* **ci:** stamp the beta version after locked tests ([8308aeb](https://github.com/anyrouter-dev/cli/commit/8308aeb3bb168ca37cb399aebd8fe0d74561fe58))
* **cli:** drop install and docs lines from --help ([0707efb](https://github.com/anyrouter-dev/cli/commit/0707efb2a41be16d7634b892426d38fd2e9fb224))
* **cli:** keep Claude opus/sonnet/haiku aliases distinct ([f81e971](https://github.com/anyrouter-dev/cli/commit/f81e971bd75bd15a54aa9a3c2fbb7f7e07ae9257))
* **cli:** login then launcher; group help ([cd8ace1](https://github.com/anyrouter-dev/cli/commit/cd8ace1bd286ac8acdb0c54057ebea5f10258371))
* **cli:** login URL includes the code and opens the browser ([a5e1307](https://github.com/anyrouter-dev/cli/commit/a5e130768a4f7bcaa17c7429ea4dc3738d9a39ab))
* **cli:** open the TUI launcher on a bare terminal run ([0508692](https://github.com/anyrouter-dev/cli/commit/05086924c74529adbb8f098de35454603e86ad22))
* **cli:** print help using the invoked command name ([16c0f1f](https://github.com/anyrouter-dev/cli/commit/16c0f1f779ac3d0ca797cff64264acc8e29ad896))
* **cli:** render a clearer AR mark ([9a2d468](https://github.com/anyrouter-dev/cli/commit/9a2d46872b7b3474ede863b57b0e601be0ba7bfb))
* **cli:** use API key for keys CRUD; drop management-key requirement ([#2](https://github.com/anyrouter-dev/cli/issues/2)) ([53e9442](https://github.com/anyrouter-dev/cli/commit/53e94422996597d2b24d8a759bf0edfa0b1f943b))
* **cli:** wrap pi with AnyRouter key and endpoint ([90246a1](https://github.com/anyrouter-dev/cli/commit/90246a160212c915617bb8b804245179e1701b4d))
* **release:** lock release-please to always-bump-patch ([4b778a2](https://github.com/anyrouter-dev/cli/commit/4b778a242bb137fc0d6fe838950e546f05923153))
* **release:** show full release-please changelog on GitHub Releases ([454e6e3](https://github.com/anyrouter-dev/cli/commit/454e6e37e4da6a46d751d82f6a88e1ddd8257263))

## [0.1.9](https://github.com/anyrouter-dev/cli/compare/v0.1.8...v0.1.9) (2026-08-21)


### Features

* **cli:** auto-update in the background by default ([f3d5edd](https://github.com/anyrouter-dev/cli/commit/f3d5edd43c3185153af4ac88647670ea7f1faff2))
* **cli:** draw a small AR mark in the terminal ([b7a313b](https://github.com/anyrouter-dev/cli/commit/b7a313b68e0bc4154f957351e59836fc7dbd23e7))
* **cli:** draw the AR mark on more terminals ([75e2a79](https://github.com/anyrouter-dev/cli/commit/75e2a79c83845cfaec0c8df446986a6d1e9be5ea))
* **cli:** publish a beta from every main push ([ff71b81](https://github.com/anyrouter-dev/cli/commit/ff71b812b056e97eefd36914406db6c5c786752b))


### Bug Fixes

* **ci:** stamp the beta version after locked tests ([8308aeb](https://github.com/anyrouter-dev/cli/commit/8308aeb3bb168ca37cb399aebd8fe0d74561fe58))
* **cli:** drop install and docs lines from --help ([0707efb](https://github.com/anyrouter-dev/cli/commit/0707efb2a41be16d7634b892426d38fd2e9fb224))
* **cli:** open the TUI launcher on a bare terminal run ([0508692](https://github.com/anyrouter-dev/cli/commit/05086924c74529adbb8f098de35454603e86ad22))
* **cli:** render a clearer AR mark ([9a2d468](https://github.com/anyrouter-dev/cli/commit/9a2d46872b7b3474ede863b57b0e601be0ba7bfb))
* **cli:** use API key for keys CRUD; drop management-key requirement ([#2](https://github.com/anyrouter-dev/cli/issues/2)) ([53e9442](https://github.com/anyrouter-dev/cli/commit/53e94422996597d2b24d8a759bf0edfa0b1f943b))
* **release:** show full release-please changelog on GitHub Releases ([454e6e3](https://github.com/anyrouter-dev/cli/commit/454e6e37e4da6a46d751d82f6a88e1ddd8257263))

## [0.1.8](https://github.com/anyrouter-dev/cli/compare/v0.1.7...v0.1.8) (2026-08-20)

### Bug Fixes

* **cli:** login URL includes the code and opens the browser ([a5e1307](https://github.com/anyrouter-dev/cli/commit/a5e130768a4f7bcaa17c7429ea4dc3738d9a39ab))

## [0.1.7](https://github.com/anyrouter-dev/cli/compare/v0.1.6...v0.1.7) (2026-08-20)

### Bug Fixes

* **cli:** keep Claude opus/sonnet/haiku aliases distinct ([f81e971](https://github.com/anyrouter-dev/cli/commit/f81e971bd75bd15a54aa9a3c2fbb7f7e07ae9257))

## [0.1.6](https://github.com/anyrouter-dev/cli/compare/v0.1.5...v0.1.6) (2026-08-20)

### Bug Fixes

* **cli:** wrap pi with AnyRouter key and endpoint ([90246a1](https://github.com/anyrouter-dev/cli/commit/90246a160212c915617bb8b804245179e1701b4d))

## [0.1.5](https://github.com/anyrouter-dev/cli/compare/v0.1.4...v0.1.5) (2026-08-20)

### Features

* **cli:** interactive TUI for ar config ([b2c7d1a](https://github.com/anyrouter-dev/cli/commit/b2c7d1a5574fe7951d37ca9073e4271196215f9a))

## [0.1.4](https://github.com/anyrouter-dev/cli/compare/v0.1.3...v0.1.4) (2026-08-20)

### Features

* **cli:** gh-style auth command group ([47563d6](https://github.com/anyrouter-dev/cli/commit/47563d6e69b7d4b84067c151d329d7c31e361436))

## [0.1.3](https://github.com/anyrouter-dev/cli/compare/v0.1.2...v0.1.3) (2026-08-20)

### Bug Fixes

* **cli:** login then launcher; group help ([cd8ace1](https://github.com/anyrouter-dev/cli/commit/cd8ace1bd286ac8acdb0c54057ebea5f10258371))

## [0.1.2](https://github.com/anyrouter-dev/cli/compare/v0.1.1...v0.1.2) (2026-08-20)

### Bug Fixes

* **ci:** resolve release bench binary by absolute path ([4291216](https://github.com/anyrouter-dev/cli/commit/429121641f56e004f7c7fbba596ffbe7eccc2b81))

## [0.1.1](https://github.com/anyrouter-dev/cli/compare/v0.1.0...v0.1.1) (2026-08-20)

### Features

* **ci:** build every platform, bench size/startup, ship wasm demo ([532a1e0](https://github.com/anyrouter-dev/cli/commit/532a1e08f31a8bcd2567cc5db2269a30ff9f27e4))
* **cli:** device login, key/model switch, agent install, color TUI ([cc4cccd](https://github.com/anyrouter-dev/cli/commit/cc4cccd144038a0454ba377648e18868e5ddc4c8))
* **cli:** spawn pi alongside claude, codex, and opencode ([56f28d0](https://github.com/anyrouter-dev/cli/commit/56f28d045c8aaa1ededc01e1155927134ffe0474))

### Bug Fixes

* **cli:** print help using the invoked command name ([16c0f1f](https://github.com/anyrouter-dev/cli/commit/16c0f1f779ac3d0ca797cff64264acc8e29ad896))
* **release:** lock release-please to always-bump-patch ([4b778a2](https://github.com/anyrouter-dev/cli/commit/4b778a242bb137fc0d6fe838950e546f05923153))

## [0.1.0](https://github.com/anyrouter-dev/cli/releases/tag/v0.1.0) (2026-08-20)

### Features

* **cli:** native anyr 0.1.x with GitHub Releases install ([836f73f](https://github.com/anyrouter-dev/cli/commit/836f73f48c3a8d48d06c1c4ea49fef9f25e83694))
