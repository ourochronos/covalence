# CompileRequest


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**compilation_focus** | **str** | Optional compilation focus hint injected into the LLM prompt. Example: \&quot;focus on architectural decisions and trade-offs\&quot;. | [optional] 
**source_ids** | **List[str]** |  | 
**title_hint** | **str** |  | [optional] 

## Example

```python
from covalence.models.compile_request import CompileRequest

# TODO update the JSON string below
json = "{}"
# create an instance of CompileRequest from a JSON string
compile_request_instance = CompileRequest.from_json(json)
# print the JSON string representation of the object
print(CompileRequest.to_json())

# convert the object into a dict
compile_request_dict = compile_request_instance.to_dict()
# create an instance of CompileRequest from a dict
compile_request_from_dict = CompileRequest.from_dict(compile_request_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


