# RecallRequest


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**context_prefix** | **str** | Only return memories whose &#x60;context&#x60; metadata field starts with this prefix.  Matched via &#x60;LIKE &#39;&lt;prefix&gt;%&#39;&#x60;, so &#x60;\&quot;session:main\&quot;&#x60; matches both &#x60;\&quot;session:main\&quot;&#x60; and &#x60;\&quot;session:main:2026-03-02\&quot;&#x60;. | [optional] 
**limit** | **int** |  | [optional] 
**min_confidence** | **float** |  | [optional] 
**query** | **str** |  | 
**since** | **str** | Only return memories created at or after this timestamp. | [optional] 
**tags** | **List[str]** |  | [optional] 

## Example

```python
from covalence.models.recall_request import RecallRequest

# TODO update the JSON string below
json = "{}"
# create an instance of RecallRequest from a JSON string
recall_request_instance = RecallRequest.from_json(json)
# print the JSON string representation of the object
print(RecallRequest.to_json())

# convert the object into a dict
recall_request_dict = recall_request_instance.to_dict()
# create an instance of RecallRequest from a dict
recall_request_from_dict = RecallRequest.from_dict(recall_request_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


