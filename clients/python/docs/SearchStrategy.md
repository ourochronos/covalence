# SearchStrategy

Adaptive fusion strategy — controls the relative weighting of the four search dimensions for different query types.  When [`SearchRequest::weights`] is explicitly provided it always takes precedence; `strategy` only applies when no explicit weights are given.

## Enum

* `BALANCED` (value: `'balanced'`)

* `PRECISE` (value: `'precise'`)

* `EXPLORATORY` (value: `'exploratory'`)

* `GRAPH` (value: `'graph'`)

* `STRUCTURAL` (value: `'structural'`)

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


