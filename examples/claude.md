# Numina Skills Configuration (claude.md)

这个文件定义了 Numina 的初始 skills（技能）。
Numina 会在启动时自动读取当前目录或 `~/.numina/workspace/claude.md`，
将其中的 `## 技能名` 段落解析为 skills，并注入到 system prompt 中。

---

## Code Review

对代码进行全面审查，包括：逻辑正确性、安全漏洞、性能问题、代码风格。

- 检查潜在的 SQL 注入、XSS、CSRF 等安全问题
- 分析时间复杂度和空间复杂度
- 提出重构建议并给出示例代码

## Refactor

将现有代码重构为更清晰、可维护的结构，遵循 SOLID 原则。

- 提取重复逻辑为独立函数或模块
- 改善命名，使代码自文档化
- 拆分过长的函数（超过 50 行建议拆分）

## Write Tests

为给定代码生成单元测试和集成测试。

- 覆盖正常路径、边界条件和错误路径
- 使用项目已有的测试框架（Rust: #[test], Python: pytest, JS: jest）
- 生成 mock/stub 以隔离外部依赖

## Explain Code

用简洁的中文解释代码的功能、设计意图和关键逻辑。

- 先给出一句话总结
- 再逐段解释关键部分
- 指出可能的坑或注意事项

## Debug

帮助定位和修复 bug，分析错误信息和堆栈跟踪。

- 分析错误信息，定位根本原因
- 提供最小复现步骤
- 给出修复方案和预防措施

## Generate Docs

为代码生成文档注释（docstring / rustdoc / JSDoc 等）。

- 描述函数/方法的功能、参数、返回值和可能的错误
- 为复杂逻辑添加行内注释
- 生成 README 或 API 文档片段

## Data Analysis

分析数据集，发现规律、异常和趋势。

- 描述数据分布和统计特征
- 识别异常值和缺失数据
- 提出可视化建议和后续分析方向
