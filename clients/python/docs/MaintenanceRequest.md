# MaintenanceRequest


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**evict_count** | **int** |  | [optional] 
**evict_if_over_capacity** | **bool** |  | [optional] 
**graph_embeddings_method** | **str** | Embedding method: &#x60;\&quot;node2vec\&quot;&#x60;, &#x60;\&quot;spectral\&quot;&#x60;, or &#x60;\&quot;both\&quot;&#x60; (default). | [optional] 
**process_queue** | **bool** |  | [optional] 
**recompute_graph_embeddings** | **bool** | When &#x60;true&#x60;, enqueue a &#x60;recompute_graph_embeddings&#x60; task. Pass &#x60;method&#x60; in the sub-field to select &#x60;\&quot;node2vec\&quot;&#x60;, &#x60;\&quot;spectral\&quot;&#x60;, or &#x60;\&quot;both\&quot;&#x60;. | [optional] 
**recompute_scores** | **bool** |  | [optional] 

## Example

```python
from covalence.models.maintenance_request import MaintenanceRequest

# TODO update the JSON string below
json = "{}"
# create an instance of MaintenanceRequest from a JSON string
maintenance_request_instance = MaintenanceRequest.from_json(json)
# print the JSON string representation of the object
print(MaintenanceRequest.to_json())

# convert the object into a dict
maintenance_request_dict = maintenance_request_instance.to_dict()
# create an instance of MaintenanceRequest from a dict
maintenance_request_from_dict = MaintenanceRequest.from_dict(maintenance_request_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


