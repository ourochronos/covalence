# SearchResult

A single search result with scores breakdown.

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**confidence** | **float** |  | 
**content_preview** | **str** |  | 
**created_at** | **str** | When this node was created. Populated from the &#x60;created_at&#x60; column on the &#x60;nodes&#x60; table; used by the &#x60;after&#x60;/&#x60;before&#x60; temporal filters. | [optional] 
**domain_path** | **List[str]** |  | [optional] 
**expanded_from** | **str** | For results returned by hierarchical expansion: the UUID of the parent article that caused this source to be included.  &#x60;None&#x60; for directly- matched results (articles or standard-mode results). | [optional] 
**graph_hops** | **int** | Number of graph hops from the nearest anchor node. &#x60;None&#x60; if this result was not discovered via graph traversal. | [optional] 
**graph_score** | **float** |  | [optional] 
**lexical_score** | **float** |  | [optional] 
**node_id** | **str** |  | 
**node_type** | **str** |  | 
**score** | **float** |  | 
**structural_score** | **float** | Structural similarity score from the graph-embedding dimension. &#x60;None&#x60; when the structural adaptor did not run or produced no result for this node (e.g. feature flag off, or no embedding in DB). | [optional] 
**title** | **str** |  | [optional] 
**topological_score** | **float** | Topological confidence score derived from graph structure (PageRank + inbound-edge diversity).  &#x60;None&#x60; when the feature flag &#x60;COVALENCE_TOPOLOGICAL_CONFIDENCE&#x60; is not enabled. | [optional] 
**trust_score** | **float** | Trustworthiness score derived from source reliability.  For source nodes this is the node&#39;s own &#x60;reliability&#x60; field. For article nodes this is the average &#x60;reliability&#x60; of all linked source nodes (via ORIGINATES / COMPILED_FROM / CONFIRMS edges). Defaults to 0.5 when no linked sources carry a reliability value. | [optional] 
**vector_score** | **float** |  | [optional] 

## Example

```python
from covalence.models.search_result import SearchResult

# TODO update the JSON string below
json = "{}"
# create an instance of SearchResult from a JSON string
search_result_instance = SearchResult.from_json(json)
# print the JSON string representation of the object
print(SearchResult.to_json())

# convert the object into a dict
search_result_dict = search_result_instance.to_dict()
# create an instance of SearchResult from a dict
search_result_from_dict = SearchResult.from_dict(search_result_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


