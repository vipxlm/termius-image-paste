# 第三方许可文本

项目源代码采用根目录 `LICENSE` 中的 MIT 许可。第三方依赖继续适用各自的许可，MIT 不覆盖第三方商标或依赖的全部权利。

发布归档应同时附带根目录 `LICENSE` 与由以下命令生成的 `THIRD_PARTY_NOTICES.txt`：

```sh
node tools/third-party-notices.mjs dist/THIRD_PARTY_NOTICES.txt
```

工具只使用 Node.js 内置模块，运行 `cargo metadata --locked`，按名称、版本和来源排序。它收集解析后的 Cargo 依赖，包含其他目标平台、构建和开发依赖，因此可能比某个二进制实际使用的范围更大。项目工作区自身的包不收集，由项目许可另行说明。

对于每个包，仅从其 manifest 所在目录读取声明的 `license_file`，以及顶层 `LICENSE`、`LICENCE`、`COPYING`、`COPYRIGHT`、`NOTICE` 和带扩展或后缀的同类文件。读取前检查路径和符号链接都没有越出该包目录。原始文本保留，超过 2 MiB、非 UTF-8 或不可读的文件明确报告；没有收集到文本的包在末尾列为 `MISSING`。

输出记录包名、版本、声明的许可表达式和来源。它不执行包中代码，不读取用户配置或密钥，也不把本机依赖路径写入输出。该清单不是法律审查，也不声称穷尽依赖源码中其他位置的所有版权或许可通知。发布维护者应查看缺失项，并保留依赖本身要求的其他声明。
