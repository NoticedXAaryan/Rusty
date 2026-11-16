# Learned optimization for Rusty RIR

This note chooses a concrete research direction for the goal of making Rusty programs at least 10% faster than the trusted reference pipeline at Clang `-O2` and `-O3`. It is a design document, not a performance claim. Measurements of the first implementation slice live with that slice; nothing here reports a speedup that has not been measured.

## What the repository actually contains

Rusty v0 is a deterministic compiler:

```text
source → lexer → parser → typed HIR → canonical RIR → LLVM IR → Clang → native
```

RIR v0 is a typed, structured, canonical S-expression (`docs/rir-v0.md`). The trusted backend lowers it to straightforward LLVM: stack slots for locals, explicit branches for `if` and `for`, and guarded `sdiv`/`srem` helpers so wrapping arithmetic never becomes LLVM undefined behavior (`docs/abi-v0.md`). Optional backends are untrusted. They may replace the reference binary only after static policy checks, sandboxed differential execution, and a bootstrap performance gate (`docs/optimization.md`).

There is no model, checkpoint, or training loop in the public tree. `research/`, `benchmarks/`, `models/`, `results/`, and `ml/` are gitignored on purpose. The neural entry point is an external process that must emit a whole LLVM module (`RUSTY_NEURAL_GENERATOR`) and a sandbox wrapper (`RUSTY_SANDBOX`). That interface is a verifier harness, not a learner.

The phrase "the Jev model, or a model like it" does not appear in the code or the docs. The architecture it matches is a **JEPA** (Joint-Embedding Predictive Architecture): predict a latent of the future state instead of generating the future state token by token (LeCun 2022; Assran et al. 2023). This note uses that meaning. A generative model that emits LLVM text is a poor fit for a compiler whose contract is that generated code is never trusted merely because it parses.

## What has to be true for a 10% claim

A result counts only if all of the following hold:

1. The candidate is semantically equivalent to the reference on the evaluated inputs (differential stdout, stderr, and exit status; traps stay traps).
2. The comparison is against the same Clang, on the same host, at `-O2` and at `-O3`, with the reference RIR→LLVM lowering left unchanged for the baseline.
3. The reported figure is a measured ratio of retained samples, not a proxy such as static instruction count. RustyBench already records medians, bootstrap intervals, and outliers (`docs/benchmarking.md`).
4. The private frozen suite, including `final-held-out`, is not used to choose the pipeline that is then scored on that split. This checkout does not contain that suite.
5. A geomean on a handful of closed scalar kernels is not a substitute for the suite. v0 programs have no inputs, no arrays, and no heap, so some of them are compile-time constants in disguise.

Published learned compilers usually clear a few percent, often on code size rather than on runtime against an already-strong `-O3`. Ten percent on a diverse runtime suite is a real target, and it will not come from swapping the Clang flag.

## Survey

### Pass ordering and autotuning

The phase-ordering problem is the classical ML-for-compilers task. Ashouri et al. (2018) survey the line of work: Milepost GCC style feature vectors, iterative compilation, and surrogate models. CompilerGym (Cummins et al. 2022) turned LLVM pass sequences into a reinforcement-learning environment. MLGO (Trofin et al. 2021) is the production-shaped version: a trained policy replaces an in-compiler heuristic. Its published pilot is inlining for size, about 7% smaller than LLVM `-Oz` on the corpora they report, not a runtime win over `-O3`. Liang et al. (ICML 2023) avoid sparse per-pass rewards by learning to pick a sequence from a coreset and report about 4.5% size reduction versus `-Oz` on unseen code. Cummins et al. (2023) train a 7B transformer to predict pass lists and auxiliary instruction counts, about 3% fewer instructions than the compiler's own choice, again a size-shaped objective.

These results are real and they transfer poorly to Rusty's goal. The reference binary is already Clang `-O3` on very simple scalar IR. Reordering LLVM passes underneath that IR can move individual programs, but the literature's typical effect is a low-single-digit size change, and the search is expensive enough that CompilerGym-style evaluation budgets are part of the method. Pass ordering remains a useful outer loop once RIR rewrites have created IR that Clang does not already normalize. It is not the first place a 10% runtime gap will come from.

### Learned cost models

Ithemal (Mendis, Renda, Amarasinghe, and Carbin, ICML 2019) predicts basic-block throughput from the instruction stream and beats analytic models such as llvm-mca on that task. IR2Vec (VenkataKeerthy et al. 2020) and ProGraML (Cummins et al. 2021) embed LLVM IR so a downstream model can rank decisions without hand-built feature lists. A cost model is necessary inside any search: equality saturation and pass selection both extract a winner by predicted cost. A cost model alone does not rewrite code. On Rusty it should score candidate RIR terms, then lose to a measurement whenever the two disagree. Static LLVM-IR cost is the wrong objective for a language whose hot path is a structured loop that Clang may or may not delete.

### LLM code generation

Generating LLVM, or asking a model to rewrite the `.ry` source, conflicts with the trust split. The 2023 LLVM-pass LLM still fails to emulate the compiler on a large fraction of programs (Cummins et al. report compilable output around 91% and exact compiler emulation around 70% on their tests). Rusty would have to reject those misses in the sandbox anyway. Token-level generation is also the opposite of what a JEPA does well: the useful prediction is "what does this program cost after this rewrite," not "what is the next line of IR."

### Superoptimization

Massalin (ASPLOS 1987) and STOKE (Schkufza, Sharma, Aiken, ASPLOS 2013) search the space of instruction sequences for shorter or faster equivalents, with a verifier or a stochastic test harness as the acceptance test. That pattern matches Rusty's untrusted-backend rule exactly. It does not match RIR. RIR has no registers and no instruction scheduling; the expensive choices are structured (loop summaries, guarded arithmetic, inlining), and Clang already schedules the LLVM that falls out of them. Superoptimization belongs on a later, lower IR if the RIR search saturates.

### Equality saturation

Equality saturation (Tate, Stepp, Tatlock, Lerner, POPL 2009) applies rewrites to an e-graph until a fixed point, then extracts a minimum-cost term. Phase ordering disappears inside the saturated set. `egg` (Willsey et al., POPL 2021) made this practical; `egglog` (Zhang, Wang, Willsey, Tatlock, PLDI 2023) unifies it with Datalog; DialEgg (Zayed and Dubach, CGO 2025) wires the same idea into MLIR. The extraction step is where a learned cost model enters, and it is allowed to be wrong because a separate checker accepts the extracted term.

This is the right rewrite engine for RIR. The IR is small, typed, structured, and canonical, which is what e-graph rewriting wants. The rewrites that can beat Clang `-O3` on v0 are identities Clang's scalar pipeline does not fully apply to *this* lowering: sums of linear recurrences that survive as vectorized loops because a remainder or a guarded division blocked closed form, and pure closed computations that are cheaper to residualize than to leave in a loop. Peephole cleanup of `x+0` will not beat `-O3`. Clang already does that, and on this backend it also deletes plain induction sums. A probe on Clang 18.1.3 (x86-64) confirmed the split: `acc += i` over a multi-million range lowers to a constant and a `printf`, while `acc += (i * 3 + 1) % 1024` and `acc += i / 7` remain real loops.

### JEPA and compiler world models

JEPA trains an encoder and a predictor in representation space. The predictor maps a context embedding plus an action to the embedding of the outcome. It does not reconstruct the outcome. LeCun's 2022 position paper argues for this as the core of a world model; I-JEPA (Assran et al. 2023) is the image instance.

CompilerDream (Deng, Wu, Feng, Wang, Long, KDD 2025; arXiv:2404.16077) is the compiler-specific world model. It learns a latent transition of LLVM IR under optimization passes, with a smoothed reward head, and uses that model either to score sequences or to propose them. The authors report a CompilerGym autotuning lead and zero-shot gains over LLVM's built-in pipelines on datasets the agent was not trained on. The training objective is still generative at the observation head. A JEPA variant drops the reconstruction of IR text and predicts only the next embedding and the scalar cost. A 2026 research program at the Lusis chair (CentraleSupélec / LISN) states that adaptation explicitly: an action-conditioned JEPA over compiler passes, with the compiler left in charge of applying and checking the chosen sequence.

That is the model class to train for Rusty, once there is something to train on. It is not a license to emit untrusted LLVM from a decoder.

## Decision

**Build a verified rewrite search over canonical RIR, and train a JEPA-style world model to rank rewrites. Do not train a generator of LLVM text.**

The search space is a coreset of semantics-preserving RIR pipelines (constant folding, single-use inlining of pure functions, closed forms for affine and periodic loop summaries, bounded partial evaluation of closed pure programs). Extraction uses a cost model whose features are RIR-level: estimated trip counts, calls, arithmetic, prints. The first cost model is a reviewed linear prior, not a trained net. The JEPA replaces that prior later. It should take the current RIR embedding and a pipeline id and predict the embedding of the rewritten RIR together with its cost. It should not predict bytes of IR.

Acceptance stays outside the model:

1. An in-process interpreter of RIR, matched to the v0 semantics, compares the candidate with the reference whenever the reference finishes inside a step budget.
2. Affine summaries additionally check a prefix of the original loop against an independent evaluation of the formula.
3. The driver compiles both programs with Clang and diffs stdout, stderr, and the exit status before the candidate may replace the reference binary. Mismatch restores the reference and records a fallback.
4. External generators keep the existing sandbox requirement. The in-tree search emits LLVM only through the trusted backend, so the dangerous surface is a wrong rewrite, which the differential check is there to catch. A passing check is still not a proof.

Why this can reach 10%, and why a pass-order policy probably cannot:

- On this lowering, Clang `-O3` already normalizes straight-line scalar code and many induction variables. A policy that only reorders LLVM passes is fighting over the residue.
- The residue is structured. Periodic sums and guarded integer division stay in loops after `-O3` (measured on Clang 18, see above). A correct closed form turns those loops into a constant. That is a large runtime change on the loop itself, not a 3% size change, and it is invisible to a pass list that never builds the identity.
- v0 programs are closed: no `argv`, no reads. Bounded partial evaluation is therefore a legal residualization, and it is the upper bound on what any optimizer can do for a program that terminates inside the budget. It will not survive a language with inputs. The JEPA has to learn the rewrites, not memorize outputs. Training data is the pipeline id and the RIR, labeled by verified cost, with `final-held-out` excluded.
- Once programs grow inputs, arrays, or a foreign-call surface, the same split holds: saturate RIR equalities, rank extracts with the world model, accept with differential tests plus the interpreter. Equality saturation (`egg` / `egglog`) replaces the hand-enumerated coreset when the rule set no longer fits in a ranked list. The cost-model interface does not change.

## Path

1. **Slice 1, in tree.** Interpreter, rewrite coreset, linear cost model, pipeline selection, native differential check, and a public development suite under `examples/kernels/`. The private RustyBench manifest stays untracked. Report medians and bootstrap intervals against `-O2` and `-O3` on that public suite only. Expect large gains where a summary deletes a loop Clang keeps, and ties where Clang already deleted it or the search falls back.
2. **Slice 2.** Replace the hand-rolled summaries with an e-graph (`egg` or `egglog`) over RIR expressions and affine loop bodies. Extraction calls the same cost-model trait. Add more identities only with interpreter and native diffs.
3. **Slice 3.** Log `(features(RIR), pipeline, features(RIR'), measured median)` from the public development split and any private train split. Train the JEPA predictor offline, outside this crate. No checkpoint is committed (the gitignore already rejects common weight formats). The compiler loads a frozen linear head or a small exported table through an explicit local path, never a network call.
4. **Slice 4.** Freeze the policy. Run the private `final-held-out` split once, with the RustyBench protocol (warmup, retained samples, bootstrap lower bound). The 10% claim is the geometric mean of median speedups versus reference `-O3`, on that split, on a named host, with the lower confidence bound also stated. If the bound is under 1.10, the claim is not met.

## What this note refuses

- A neural-performance claim for the current `rusty optimize` path. No generator is shipped.
- Training on, or tuning thresholds against, `final-held-out`.
- Treating compile-time execution of closed v0 programs as evidence that a learned model beat Clang on general code. It is a legal optimization and a useful upper bound. It is not the JEPA result.
- Copying private suites, datasets, or weights into git.

## References

- Amir H. Ashouri, William Killian, John Cavazos, Gianluca Palermo, and Cristina Silvano. "A Survey on Compiler Autotuning using Machine Learning." *ACM Computing Surveys* 51(5), 2018. <https://doi.org/10.1145/3197978>
- Mahmoud Assran, Quentin Duval, Ishan Misra, Piotr Bojanowski, Pascal Vincent, Michael Rabbat, Yann LeCun, and Nicolas Ballas. "Self-Supervised Learning from Images with a Joint-Embedding Predictive Architecture." CVPR 2023. arXiv:2301.08243. <https://arxiv.org/abs/2301.08243>
- Chris Cummins, Zacharias V. Fisches, Tal Ben-Nun, Torsten Hoefler, Michael F. P. O'Boyle, and Hugh Leather. "ProGraML: A Graph-based Program Representation for Data Flow Analysis and Compiler Optimizations." ICML 2021. arXiv:2003.10536. <https://arxiv.org/abs/2003.10536>
- Chris Cummins, Bram Wasti, Jiadong Guo, Brandon Cui, Jason Ansel, Sahir Gomez, Somya Jain, Jia Liu, Olivier Teytaud, Benoit Steiner, Yuandong Tian, and Hugh Leather. "CompilerGym: Robust, Performant Compiler Optimization Environments for AI Research." CGO 2022. arXiv:2109.08267. <https://arxiv.org/abs/2109.08267>
- Chris Cummins, Volker Seeker, Dejan Grubisic, Mostafa Elhoushi, Youwei Liang, Baptiste Roziere, Jonas Gehring, Fabian Gloeckle, Kim Hazelwood, Gabriel Synnaeve, and Hugh Leather. "Large Language Models for Compiler Optimization." 2023. arXiv:2309.07062. <https://arxiv.org/abs/2309.07062>
- Chaoyi Deng, Jialong Wu, Ningya Feng, Jianmin Wang, and Mingsheng Long. "CompilerDream: Learning a Compiler World Model for General Code Optimization." KDD 2025. arXiv:2404.16077. <https://arxiv.org/abs/2404.16077>
- Yann LeCun. "A Path Towards Autonomous Machine Intelligence." OpenReview, 2022. <https://openreview.net/forum?id=BZ5a1r-kVsf>
- Youwei Liang, Kevin Stone, Ali Shameli, Chris Cummins, Mostafa Elhoushi, Jiadong Guo, Benoit Steiner, Xiaomeng Yang, Pengtao Xie, Hugh Leather, and Yuandong Tian. "Learning Compiler Pass Orders using Coreset and Normalized Value Prediction." ICML 2023. <https://proceedings.mlr.press/v202/liang23f.html>
- Henry Massalin. "Superoptimizer: A Look at the Smallest Program." ASPLOS 1987. <https://doi.org/10.1145/36206.36194>
- Charith Mendis, Alex Renda, Saman Amarasinghe, and Michael Carbin. "Ithemal: Accurate, Portable and Fast Basic Block Throughput Estimation using Deep Neural Networks." ICML 2019. <https://proceedings.mlr.press/v97/mendis19a.html>
- Eric Schkufza, Rahul Sharma, and Alex Aiken. "Stochastic Superoptimization." ASPLOS 2013. <https://doi.org/10.1145/2451116.2451150>
- Ross Tate, Michael Stepp, Zachary Tatlock, and Sorin Lerner. "Equality Saturation: A New Approach to Optimization." POPL 2009. <https://doi.org/10.1145/1480881.1480915>
- Mircea Trofin, Yundi Qian, Eugene Brevdo, Zinan Lin, Krzysztof Choromanski, and David Li. "MLGO: a Machine Learning Guided Compiler Optimizations Framework." 2021. arXiv:2101.04808. <https://arxiv.org/abs/2101.04808>
- S. VenkataKeerthy, Rohit Aggarwal, Shalini Jain, Maunendra Sankar Desarkar, Ramakrishna Upadrasta, and Y. N. Srikant. "IR2Vec: LLVM IR Based Scalable Program Embeddings." *ACM TACO* 17(4), 2020. arXiv:1909.06228. <https://arxiv.org/abs/1909.06228>
- Max Willsey, Chandrakana Nandi, Yisu Remy Wang, Oliver Flatt, Zachary Tatlock, and Pavel Panchekha. "egg: Fast and Extensible Equality Saturation." POPL 2021. arXiv:2004.03082. <https://arxiv.org/abs/2004.03082>
- Yihong Zhang, Yisu Remy Wang, Oliver Flatt, David Cao, Philip Zucker, Eli Rosenthal, Zachary Tatlock, and Max Willsey. "Better Together: Unifying Datalog and Equality Saturation." PLDI 2023. arXiv:2304.04332. <https://arxiv.org/abs/2304.04332>
- Abd-El-Aziz Zayed and Christophe Dubach. "DialEgg: Dialect-Agnostic MLIR Optimizer using Equality Saturation with Egglog." CGO 2025. <https://doi.org/10.1145/3696443.3708957>
- Lusis chair / LISN. "JEPA for Code Generation and Optimization" (2026–2027 research initiation). <https://chaire-lusis.centralesupelec.fr/graduate/2026/2026-jepa-symbolic-world-models/>
