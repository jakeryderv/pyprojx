# Changelog

## [0.5.0](https://github.com/jakeryderv/pyprojx/compare/v0.4.0...v0.5.0) (2026-09-24)


### Features

* check [tool.ty] options, values, and rules against the allowed ty versions ([#26](https://github.com/jakeryderv/pyprojx/issues/26)) ([a60d049](https://github.com/jakeryderv/pyprojx/commit/a60d049ba7430f60aba8f451f6a827133f1adc54))
* check [tool.uv.build-backend] against the uv_build versions [build-system] allows ([#30](https://github.com/jakeryderv/pyprojx/issues/30)) ([5949266](https://github.com/jakeryderv/pyprojx/commit/59492664c98c86fc1159bf5be999104673656c0d))
* check [tool.uv] options and values against the allowed uv versions ([#28](https://github.com/jakeryderv/pyprojx/issues/28)) ([77549f2](https://github.com/jakeryderv/pyprojx/commit/77549f239b10f99fbcea146bacb61d2c478a5fc5))
* check the names [tool.uv] refers to and the shape of its sources ([#29](https://github.com/jakeryderv/pyprojx/issues/29)) ([c9e1283](https://github.com/jakeryderv/pyprojx/commit/c9e1283fa20ce830fb0ce3617caba46243aac767))

## [0.4.0](https://github.com/jakeryderv/pyprojx/compare/v0.3.0...v0.4.0) (2026-09-24)


### Features

* check [tool.ruff] options against the allowed Ruff versions ([#19](https://github.com/jakeryderv/pyprojx/issues/19)) ([db49acb](https://github.com/jakeryderv/pyprojx/commit/db49acbbc3533d433b01dfe1c537ead375f0ffa2))
* check Ruff option values against the allowed versions ([#22](https://github.com/jakeryderv/pyprojx/issues/22)) ([d2c34b0](https://github.com/jakeryderv/pyprojx/commit/d2c34b00c318051a96c9b6647dee7bdc8a9ca96f))
* check Ruff rule selectors against the allowed versions ([#21](https://github.com/jakeryderv/pyprojx/issues/21)) ([3bdb885](https://github.com/jakeryderv/pyprojx/commit/3bdb88533b7324ef5576251ebe65d566e29eafc8))

## [0.3.0](https://github.com/jakeryderv/pyprojx/compare/v0.2.0...v0.3.0) (2026-09-24)


### Features

* check PEP 794, PEP 808, and license classifiers against the backend ([#17](https://github.com/jakeryderv/pyprojx/issues/17)) ([87d8401](https://github.com/jakeryderv/pyprojx/commit/87d8401cf2e70de89ba3fbba9e7b0f8249046fb8))
* report features the build backend versions lack ([#15](https://github.com/jakeryderv/pyprojx/issues/15)) ([dc831c3](https://github.com/jakeryderv/pyprojx/commit/dc831c3937456f0da27ff7262e3d0b24314e83ea))

## [0.2.0](https://github.com/jakeryderv/pyprojx/compare/v0.1.0...v0.2.0) (2026-09-24)


### Features

* validate [build-system] ([#9](https://github.com/jakeryderv/pyprojx/issues/9)) ([266bc08](https://github.com/jakeryderv/pyprojx/commit/266bc08c7a8d7360c3b2cfc6bd70533f8dc1903e))
* validate [dependency-groups] ([#13](https://github.com/jakeryderv/pyprojx/issues/13)) ([5951a6e](https://github.com/jakeryderv/pyprojx/commit/5951a6e4e180604d910be26a99cd763d1a8c8813))
* validate project dependencies and license metadata ([#12](https://github.com/jakeryderv/pyprojx/issues/12)) ([b1a1a7d](https://github.com/jakeryderv/pyprojx/commit/b1a1a7d00d7ce1ad546765c817d3d8ed9a4e5497))
* validate the structure of [project] ([#11](https://github.com/jakeryderv/pyprojx/issues/11)) ([76371b9](https://github.com/jakeryderv/pyprojx/commit/76371b9a11a406b0de899b3f73c1c38620d91285))
* validate trove classifiers ([#14](https://github.com/jakeryderv/pyprojx/issues/14)) ([aaa6a9b](https://github.com/jakeryderv/pyprojx/commit/aaa6a9bb0ea08f61e1abac8d8c1b2769b83329cd))

## [0.1.0](https://github.com/jakeryderv/pyprojx/compare/v0.0.1...v0.1.0) (2026-09-24)


### Features

* **cli:** add pyprojx check ([#7](https://github.com/jakeryderv/pyprojx/issues/7)) ([f9fb12b](https://github.com/jakeryderv/pyprojx/commit/f9fb12bb9b70d3af3fc2d9997685bd40bdf4ad6b))
* **core:** collect TOML syntax errors with source spans ([#5](https://github.com/jakeryderv/pyprojx/issues/5)) ([f1c16a8](https://github.com/jakeryderv/pyprojx/commit/f1c16a86ca0bc4b7b3b8845ed8f676e062f7e322))
* warn about TOML 1.1-only syntax ([#8](https://github.com/jakeryderv/pyprojx/issues/8)) ([1136765](https://github.com/jakeryderv/pyprojx/commit/11367654c0b09bc0bb2b0c03392894adc2fc7a7a))
