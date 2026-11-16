# Learned optimization for Rusty RIR

This note records what the literature actually supports, the order in which Rusty should try to beat Clang, and the measurement bar a 10% claim has to clear. It is not itself a performance claim. Numbers from a Rusty run appear only in `docs/research/measurements.md`, and only after they have been measured.

## What is in the tree

```text
source → lexer → parser → typed HIR → canonical RIR → LLVM IR → Clang → native
```

RIR v0 is a typed, canonical S-expression (`docs/rir-v0.md`). The trusted backend lowers it with stack slots, explicit branches, and guarded division so wrapping arithmetic never becomes LLVM undefined behavior (`docs/abi-v0.md`). Optional backends are untrusted. The external generator path still requires `RUSTY_NEURAL_GENERATOR` and `RUSTY_SANDBOX` (`docs/optimization.md`). Nothing in the public tree trains a model. `research/`, `benchmarks/`, `models/`, and `results/` stay gitignored.

"Jev" does not appear in the code. The architecture it matches is a JEPA: predict an embedding of a future state, not the state itself (LeCun 2022; Assran et al., I-JEPA, arXiv:2301.08243). No JEPA-for-compilers paper has been published. LLM-JEPA (Huang, LeCun, and Balestriero, arXiv:2509.14252) is a language-model training objective, not a compiler. A student project at the Lusis chair has proposed an action-conditioned JEPA over compiler passes; that is a proposal, not a result. CompilerDream (Deng et al., arXiv:2404.16077) is a published compiler world model, and it still reconstructs observations. It is adjacent prior art, not a JEPA.

## What published systems actually deliver

On general code, learned compilers beat a strong `-O2`/`-O3` by a small amount:

- MLGO's deployed register allocator improves queries per second by about 0.3% to 1.5% on Google datacenter binaries (Trofin et al., arXiv:2101.04808, and the MLGO register-allocation deployment). The same line of work's better-known inlining result is a code-size win against `-Oz`, not a runtime win against `-O3`.
- Minotaur, a synthesizing superoptimizer checked by Alive2, reports about 1.5% average on SPEC CPU 2017 and 7.3% average (13% max) on GMP's own suite (Liu, Mada, and Regehr, arXiv:2306.00229, PACMPL OOPSLA 2024).
- Stabilizer showed that a large part of the apparent gap between `-O3` and `-O2` is layout noise once code and heap placement are randomized (Curtsinger and Berger, ASPLOS 2013). On a general suite, `-O3` versus `-O2` is often inside the noise.

Double-digit gains show up in narrow domains, not as a default:

- Tensat's equality saturation on tensor graphs reports speedups up to 16% (Yang et al., arXiv:2101.01332).
- AutoPhase reports a 28% circuit-performance gain against `-O3` on high-level synthesis, which is a different objective from CPU runtime (Haj-Ali et al., MLSys 2020).

Rusty's reference is already Clang `-O3` on very small scalar IR. A pass-order policy aimed at general LLVM is the wrong place to expect 10%. The realistic 10% is a domain rewrite Clang does not perform on this lowering: closed forms for range loops that stay as loops after `-O3`, and dropping a guarded division once the divisor is proven safe. A probe on Clang 18.1.3 (x86-64) showed `acc += i` over a multi-million range collapsing to a constant, while `acc += (i * 3 + 1) % 1024` and `acc += i / 7` remained loops.

The in-tree performance gate still defaults to 2% (`--minimum-speedup`, used by the external generator path). Leave that default. Anything we describe as progress toward the research goal is reported against 10%: the bootstrap lower bound on median(`-O3`) / median(candidate) must be at least 1.10. A point estimate above 1.10 with a lower bound under 1.10 is not a 10% result.

## Measurement bar

No speedup is a result until all of the following are true for that run:

1. The public development suite (`examples/kernels/suite.tsv`) or a private manifest is frozen: source checksums checked, and the manifest bytes themselves pinned. The private `final-held-out` split is not in this checkout and is not used to pick a pipeline.
2. Baselines are reference Clang `-O2`, `-O3`, and `-O3 -march=native`, same compiler, same host. The candidate is compiled at the same flag as the baseline it is compared with.
3. Link order and environment size are randomized across samples, so a layout accident is not reported as a win (the Stabilizer lesson).
4. Kernels that win on a discovery run are timed again on a fresh run. The fresh run is the number that is published. The discovery run is not.
5. Equivalence is differential, not "it compiled". The interpreter compares candidates that finish inside the step budget. A native diff compares stdout, stderr, and exit status. A fuzzer generates random v0 programs and checks the same. Alive2 (Lopes et al., PLDI 2021, [doi:10.1145/3453483.3454030](https://doi.org/10.1145/3453483.3454030)) is the LLVM-level checker to run when `alive-tv` is on `PATH`. If it is not installed, the run must say so, and the LLVM-level rewrite is not called formally verified.

v0 programs do not read input. A closed-form or a partial evaluation that deletes a loop is a legal optimization on this language and a poor predictor of a language with inputs. Say which case a number is.

## Ranked directions

### 1. Equality saturation over RIR (implement first)

Saturate semantics-preserving rewrites in an e-graph and extract a minimum-cost term. Phase ordering inside the saturated set goes away (Tate et al., POPL 2009). `egg` (Willsey et al., arXiv:2004.03082) is the library. `egglog` (Zhang et al., arXiv:2304.04332) is the version to move to when the side conditions want Datalog (interval facts, "this divisor is nonzero"). Tensat is the evidence that extraction against a cost model can produce double-digit speedups when the domain has identities the general compiler misses.

Rusty-specific rules, the ones LLVM does not already apply to this lowering:

- Guarded-division elimination. `x / C` and `x % C` become raw `sdiv` / `srem` only when `C` is a constant other than `0` and `-1`. The only overflow case in v0 is `INT64_MIN / -1`. A variable divisor keeps the guard.
- Closed forms for accumulator loops whose body is affine, a positive remainder of an affine form, or `i / C`, and only when an `i128` proof shows the wrapping loop matches the formula.
- Ordinary algebraic cleanup (`x+0`, `x*1`, `x/1`) so the e-class actually contains the simpler term.

The cost function prefers a constant to a loop and a raw division to a guarded one. Extraction can be wrong about performance. It must not be the equivalence proof. The interpreter, the native diff, and the fuzzer are the proof we have. The rules are sound only as far as those checks and the `i128` side conditions hold.

### 2. Per-program search over passes, flags, and inline/unroll

After direction 1 has changed the IR, search a CompilerGym-style space: LLVM pass order, a small flag set, inlining, unrolling (Cummins et al., arXiv:2109.08267). Rank candidates with a learned throughput model in the style of Ithemal (Mendis et al., arXiv:1808.07412) rather than with another hand-written heuristic. MLGO (arXiv:2101.04808) is the production-shaped version of "replace one heuristic". Minotaur (arXiv:2306.00229) is the reminder that even a verified superoptimizer's average on SPEC is about 1.5%. This direction is second because that is the effect size to expect from it on general code. It is still worth building once RIR rewrites have created IR that `-O3` does not already normalize. It is not implemented yet.

### 3. JEPA pretraining, judged only as a cost model

Pretrain an encoder on RIR and LLVM IR by predicting embeddings of masked subgraphs (the I-JEPA objective, arXiv:2301.08243, with ProGraML's graph view of IR, arXiv:2003.10536, as the natural input). LLM-JEPA (arXiv:2509.14252) is evidence that the same objective can be stated for token models; it is not a compiler result. The encoder is accepted only if it improves the cost models in directions 1 and 2 on a frozen split the encoder was not trained on. A prettier embedding with no change in extracted code or in measured time is not progress. No encoder is trained in this tree.

## What this note refuses

- Calling the external generator path a neural result. No generator is shipped.
- Moving the 2% gate. The research report uses 10%.
- Training or tuning on `final-held-out`.
- Treating a geomean on closed scalar kernels as a SPEC-style claim. Those kernels are the development set named in the measurement note.
- Publishing a discovery run that has not been remeasured under layout randomization.

## References

- Mahmoud Assran et al. "Self-Supervised Learning from Images with a Joint-Embedding Predictive Architecture." CVPR 2023. arXiv:2301.08243.
- Charlie Curtsinger and Emery D. Berger. "Stabilizer: Statistically Sound Performance Evaluation." ASPLOS 2013. <https://doi.org/10.1145/2451116.2451141>
- Chris Cummins et al. "CompilerGym: Robust, Performant Compiler Optimization Environments for AI Research." CGO 2022. arXiv:2109.08267.
- Chris Cummins et al. "ProGraML: A Graph-based Program Representation for Data Flow Analysis and Compiler Optimizations." ICML 2021. arXiv:2003.10536.
- Chaoyi Deng et al. "CompilerDream: Learning a Compiler World Model for General Code Optimization." KDD 2025. arXiv:2404.16077.
- Ameer Haj-Ali, Qijing Huang, William Moses, John Xiang, Krste Asanović, John Wawrzynek, and Ion Stoica. "AutoPhase: Juggling HLS Phase Orderings in Random Forests with Deep Reinforcement Learning." MLSys 2020.
- Hai Huang, Yann LeCun, and Randall Balestriero. "LLM-JEPA: Large Language Models Meet Joint Embedding Predictive Architectures." arXiv:2509.14252.
- Yann LeCun. "A Path Towards Autonomous Machine Intelligence." 2022. <https://openreview.net/forum?id=BZ5a1r-kVsf>
- Zhengyang Liu, Stefan Mada, and John Regehr. "Minotaur: A SIMD-Oriented Synthesizing Superoptimizer." PACMPL OOPSLA 2024. arXiv:2306.00229. <https://doi.org/10.1145/3689766>
- Nuno P. Lopes, Juneyoung Lee, Chung-Kil Hur, Zhengyang Liu, and John Regehr. "Alive2: Bounded Translation Validation for LLVM." PLDI 2021. <https://doi.org/10.1145/3453483.3454030>
- Charith Mendis, Alex Renda, Saman Amarasinghe, and Michael Carbin. "Ithemal: Accurate, Portable and Fast Basic Block Throughput Estimation using Deep Neural Networks." ICML 2019. arXiv:1808.07412.
- Ross Tate, Michael Stepp, Zachary Tatlock, and Sorin Lerner. "Equality Saturation: A New Approach to Optimization." POPL 2009. <https://doi.org/10.1145/1480881.1480915>
- Mircea Trofin et al. "MLGO: a Machine Learning Guided Compiler Optimizations Framework." arXiv:2101.04808. Register-allocation deployment: <https://research.google/blog/mlgo-a-machine-learning-framework-for-compiler-optimization/>
- Max Willsey et al. "egg: Fast and Extensible Equality Saturation." POPL 2021. arXiv:2004.03082.
- Yichen Yang, Phitchaya Mangpo Phothilimthana, Yisu Remy Wang, Max Willsey, Sudip Roy, and Jacques Pienaar. "Equality Saturation for Tensor Graph Superoptimization." MLSys 2021. arXiv:2101.01332.
- Yihong Zhang et al. "Better Together: Unifying Datalog and Equality Saturation." PLDI 2023. arXiv:2304.04332.
