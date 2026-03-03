# QueueStats


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**completed_24h** | **int** |  | 
**failed** | **int** |  | 
**pending** | **int** |  | 
**processing** | **int** |  | 

## Example

```python
from covalence.models.queue_stats import QueueStats

# TODO update the JSON string below
json = "{}"
# create an instance of QueueStats from a JSON string
queue_stats_instance = QueueStats.from_json(json)
# print the JSON string representation of the object
print(QueueStats.to_json())

# convert the object into a dict
queue_stats_dict = queue_stats_instance.to_dict()
# create an instance of QueueStats from a dict
queue_stats_from_dict = QueueStats.from_dict(queue_stats_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


