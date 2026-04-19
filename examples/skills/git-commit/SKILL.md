---
description: 根据 git diff 自动生成规范的 commit message
when_to_use: 当用户说"帮我写 commit"、"生成 commit message"、"commit 信息"、"提交说明"、"write commit message" 时
argument-hint: "<branch_or_diff>"
---

请根据当前的 git 变更生成一个规范的 commit message。

$ARGUMENT

## 步骤

1. 先运行 `git diff --staged` 查看暂存区变更（如果为空，运行 `git diff HEAD` 查看所有变更）
2. 分析变更内容，理解改动的目的
3. 生成符合 Conventional Commits 规范的 commit message

## Commit Message 格式

```
<type>(<scope>): <subject>

[optional body]

[optional footer]
```

### Type 类型
- `feat`: 新功能
- `fix`: Bug 修复
- `docs`: 文档更新
- `style`: 代码格式（不影响功能）
- `refactor`: 重构（不是新功能也不是 bug 修复）
- `perf`: 性能优化
- `test`: 测试相关
- `chore`: 构建/工具链/依赖更新
- `ci`: CI/CD 配置

### 规则
- subject 使用祈使句，首字母小写，不加句号
- subject 不超过 72 个字符
- body 说明"为什么"而不是"做了什么"
- 如果有 breaking change，在 footer 加 `BREAKING CHANGE: <description>`

## 输出

直接输出可以使用的 commit message，不需要额外解释。
如果变更较多，提供 2-3 个候选方案供选择。
