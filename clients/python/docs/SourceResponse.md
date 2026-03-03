# SourceResponse

Response envelope for a source.

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**capture_method** | **str** | How this source was captured — read from &#x60;metadata[\&quot;capture_method\&quot;]&#x60;. &#x60;None&#x60; when absent (e.g. sources ingested before this field existed). | [optional] 
**confidence** | **float** |  | 
**content** | **str** |  | 
**created_at** | **str** |  | 
**fingerprint** | **str** |  | 
**id** | **str** |  | 
**metadata** | **object** |  | 
**modified_at** | **str** |  | 
**node_type** | **str** |  | 
**reliability** | **float** |  | 
**source_type** | **str** |  | [optional] 
**status** | **str** |  | 
**title** | **str** |  | [optional] 
**version** | **int** |  | 

## Example

```python
from covalence.models.source_response import SourceResponse

# TODO update the JSON string below
json = "{}"
# create an instance of SourceResponse from a JSON string
source_response_instance = SourceResponse.from_json(json)
# print the JSON string representation of the object
print(SourceResponse.to_json())

# convert the object into a dict
source_response_dict = source_response_instance.to_dict()
# create an instance of SourceResponse from a dict
source_response_from_dict = SourceResponse.from_dict(source_response_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


