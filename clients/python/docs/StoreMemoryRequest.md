# StoreMemoryRequest


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**content** | **str** |  | 
**context** | **str** |  | [optional] 
**importance** | **float** |  | [optional] 
**supersedes_id** | **str** |  | [optional] 
**tags** | **List[str]** |  | [optional] 

## Example

```python
from covalence.models.store_memory_request import StoreMemoryRequest

# TODO update the JSON string below
json = "{}"
# create an instance of StoreMemoryRequest from a JSON string
store_memory_request_instance = StoreMemoryRequest.from_json(json)
# print the JSON string representation of the object
print(StoreMemoryRequest.to_json())

# convert the object into a dict
store_memory_request_dict = store_memory_request_instance.to_dict()
# create an instance of StoreMemoryRequest from a dict
store_memory_request_from_dict = StoreMemoryRequest.from_dict(store_memory_request_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


