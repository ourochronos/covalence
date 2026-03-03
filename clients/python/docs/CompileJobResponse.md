# CompileJobResponse


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**job_id** | **str** |  | 
**status** | **str** |  | 

## Example

```python
from covalence.models.compile_job_response import CompileJobResponse

# TODO update the JSON string below
json = "{}"
# create an instance of CompileJobResponse from a JSON string
compile_job_response_instance = CompileJobResponse.from_json(json)
# print the JSON string representation of the object
print(CompileJobResponse.to_json())

# convert the object into a dict
compile_job_response_dict = compile_job_response_instance.to_dict()
# create an instance of CompileJobResponse from a dict
compile_job_response_from_dict = CompileJobResponse.from_dict(compile_job_response_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


