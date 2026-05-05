# provenance

Canonical byte-offset sourcing, the Offset Projection Ledger, provenance-link integrity. Every node, edge, and chunk traces back to immutable byte offsets in the source text (INV-2).

Mutated text (e.g., after fastcoref coreference) is reverse-projected through the ledger before storage. Synthetic facts (deduced edges, consolidated nodes) link to their derivation source.

Apply this concern when any code path creates or transforms a node/edge/chunk; default to `n/a` only if the module touches no graph artifacts.
