# Lab 3：进程与 Stride 调度

## 实现内容

- `spawn` 直接从 ELF 创建全新地址空间，只建立父子关系，不复制父进程内存。
- 为任务保存 `priority` 与累计 `stride`，优先级必须大于等于 2。
- 调度器每次选择 stride 最小的就绪任务，并按 `BigStride / priority` 增加其 stride。

## 溢出比较

若直接比较无符号整数，stride 回绕后会把刚运行过的任务误判为最小值。优先级至少为 2 时，合法任务之间的 stride 距离始终小于半个整数取值范围，因此可把 `a.wrapping_sub(b)` 解释为有符号数：结果为负表示 `a` 在环形序列上早于 `b`。调度器使用这一比较方式，既不需要扩大计数类型，也能在整数回绕后保持顺序。

`BigStride` 取 `usize::MAX / 2`。这样最小合法优先级的单次增量不超过四分之一取值范围，而最大合法 `isize` 优先级仍获得非零增量。

## 验证

本地使用 RISC-V 裸机目标执行格式检查和 `cargo check`；spawn、wait、优先级边界和六任务权重比例由 Chapter 5 官方 Checker 验证。
