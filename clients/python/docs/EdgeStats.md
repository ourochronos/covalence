# EdgeStats


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**age_count** | **int** |  | 
**in_sync** | **bool** |  | 
**sql_count** | **int** |  | 

## Example

```python
from covalence.models.edge_stats import EdgeStats

# TODO update the JSON string below
json = "{}"
# create an instance of EdgeStats from a JSON string
edge_stats_instance = EdgeStats.from_json(json)
# print the JSON string representation of the object
print(EdgeStats.to_json())

# convert the object into a dict
edge_stats_dict = edge_stats_instance.to_dict()
# create an instance of EdgeStats from a dict
edge_stats_from_dict = EdgeStats.from_dict(edge_stats_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


