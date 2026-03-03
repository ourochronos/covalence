# EmbeddingStats


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**nodes_without** | **int** |  | 
**total** | **int** |  | 

## Example

```python
from covalence.models.embedding_stats import EmbeddingStats

# TODO update the JSON string below
json = "{}"
# create an instance of EmbeddingStats from a JSON string
embedding_stats_instance = EmbeddingStats.from_json(json)
# print the JSON string representation of the object
print(EmbeddingStats.to_json())

# convert the object into a dict
embedding_stats_dict = embedding_stats_instance.to_dict()
# create an instance of EmbeddingStats from a dict
embedding_stats_from_dict = EmbeddingStats.from_dict(embedding_stats_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


