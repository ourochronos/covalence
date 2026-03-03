# MaintenanceResponse


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**actions_taken** | **List[str]** |  | 

## Example

```python
from covalence.models.maintenance_response import MaintenanceResponse

# TODO update the JSON string below
json = "{}"
# create an instance of MaintenanceResponse from a JSON string
maintenance_response_instance = MaintenanceResponse.from_json(json)
# print the JSON string representation of the object
print(MaintenanceResponse.to_json())

# convert the object into a dict
maintenance_response_dict = maintenance_response_instance.to_dict()
# create an instance of MaintenanceResponse from a dict
maintenance_response_from_dict = MaintenanceResponse.from_dict(maintenance_response_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


