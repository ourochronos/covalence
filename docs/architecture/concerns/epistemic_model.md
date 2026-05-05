# epistemic_model

Subjective Logic opinion tuples (b, d, u, a), confidence propagation, fusion rules (Dempster-Shafer, DF-QuAD), decay (BMR forgetting), convergence detection.

Uncertainty is not disbelief (INV-6). Operations that collapse opinion tuples to point estimates require explicit justification; decay raises uncertainty (`u`), not skepticism (`d`).

Apply when: any code path computes, stores, or reasons over confidence values. Default to `n/a` only when the module operates strictly on metadata (paths, names, types) without confidence.
