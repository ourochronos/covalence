# Memory


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**confidence** | **float** |  | 
**content** | **str** |  | 
**context** | **str** |  | [optional] 
**created_at** | **str** |  | 
**forgotten** | **bool** |  | 
**id** | **str** |  | 
**importance** | **float** |  | 
**tags** | **object** |  | 

## Example

```python
from covalence.models.memory import Memory

# TODO update the JSON string below
json = "{}"
# create an instance of Memory from a JSON string
memory_instance = Memory.from_json(json)
# print the JSON string representation of the object
print(Memory.to_json())

# convert the object into a dict
memory_dict = memory_instance.to_dict()
# create an instance of Memory from a dict
memory_from_dict = Memory.from_dict(memory_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


