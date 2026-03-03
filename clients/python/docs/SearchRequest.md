# SearchRequest

Request body for POST /search.

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**after** | **str** | Include only nodes whose &#x60;created_at&#x60; is strictly after this timestamp. When &#x60;None&#x60;, no lower-bound date filter is applied. | [optional] 
**before** | **str** | Include only nodes whose &#x60;created_at&#x60; is strictly before this timestamp. When &#x60;None&#x60;, no upper-bound date filter is applied. | [optional] 
**domain_path** | **List[str]** | Optional domain-path filter. When set, only nodes whose &#x60;domain_path&#x60; array shares at least one element with this list are returned. Nodes with an empty or NULL &#x60;domain_path&#x60; are excluded when filtering. | [optional] 
**embedding** | **List[float]** |  | [optional] 
**intent** | [**SearchIntent**](SearchIntent.md) |  | [optional] 
**limit** | **int** |  | [optional] 
**max_hops** | **int** | Maximum graph traversal hops (1-3, default 1). Higher values discover structurally distant but related nodes via edge chains.  When absent, the effective default is determined by &#x60;strategy&#x60;: - [&#x60;SearchStrategy::Graph&#x60;] → 2 hops - All other strategies → 1 hop  Explicit &#x60;max_hops&#x60; always wins over the strategy-derived default. | [optional] 
**min_score** | **float** | Minimum score threshold (covalence#33). Results with a final score strictly below this value are filtered out before returning. When &#x60;None&#x60; (the default), all results are returned regardless of score. Useful for precision queries: set to 0.6 to avoid weak matches. | [optional] 
**mode** | [**SearchMode**](SearchMode.md) | Search mode — defaults to [&#x60;SearchMode::Standard&#x60;]. | [optional] 
**node_types** | **List[str]** |  | [optional] 
**query** | **str** |  | 
**recency_bias** | **float** | Recency bias factor (0.0–1.0). Higher values favor newer content. At 0.0 (default), freshness gets 5% weight (current behavior). At 1.0, freshness gets 40% weight (strongly favor recent). | [optional] 
**session_id** | **str** |  | [optional] 
**strategy** | [**SearchStrategy**](SearchStrategy.md) | Search strategy — adjusts dimension weights for different query types. Overrides &#x60;weights&#x60; when set to a non-&#x60;Balanced&#x60; value. When both &#x60;weights&#x60; and &#x60;strategy&#x60; are provided, &#x60;weights&#x60; wins. | [optional] 
**weights** | [**WeightsInput**](WeightsInput.md) |  | [optional] 

## Example

```python
from covalence.models.search_request import SearchRequest

# TODO update the JSON string below
json = "{}"
# create an instance of SearchRequest from a JSON string
search_request_instance = SearchRequest.from_json(json)
# print the JSON string representation of the object
print(SearchRequest.to_json())

# convert the object into a dict
search_request_dict = search_request_instance.to_dict()
# create an instance of SearchRequest from a dict
search_request_from_dict = SearchRequest.from_dict(search_request_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


