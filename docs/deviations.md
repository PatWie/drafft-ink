# Deviations from the plan

A running log of every place where the work departs from `docs/plan.md`, with the reason. The final report lists all of them.

| # | Date | Plan said | We do | Why |
|---|---|---|---|---|
| D1 | 2026-11-07 | Native shell on eframe (§10.3) | One own shell on winit + egui-winit + egui-wgpu for native and web. eframe stays out of the app. | The stack spike measured eframe's web runner at 3.8-4.2% of a core while idle, because it keeps a requestAnimationFrame loop alive. An own winit shell idles at 0.7-1.2% with the same wasm size (docs/stack.md). "Idle means idle" is a hard rule, and one shell for both targets avoids two code paths. Cost: the shell supplies IME, clipboard, file drop and touch on the web itself. |
| D2 | 2026-11-07 | `collab` at L5 next to `engine`; `SceneStore` trait in `canvas` | `collab` at L4; `SceneStore` in `scene` (L1) | Removes both same-layer exceptions, so the layering has no holes. |
| D3 | 2026-11-07 | A panic inside a command rolls back (§5) | Native only. On wasm (panic=abort) a panic hook reports and offers a reload from the last autosave. | `catch_unwind` needs unwinding, which wasm32-unknown-unknown lacks. |
