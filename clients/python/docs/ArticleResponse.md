# ArticleResponse


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**confidence** | **float** |  | 
**content** | **str** |  | [optional] 
**contention_count** | **int** |  | 
**created_at** | **str** |  | 
**domain_path** | **List[str]** |  | 
**epistemic_type** | **str** |  | [optional] 
**id** | **str** |  | 
**metadata** | **object** |  | 
**modified_at** | **str** |  | 
**node_type** | **str** |  | 
**pinned** | **bool** |  | 
**stale** | **bool** | &#x60;true&#x60; when newer unlinked sources exist in the same domain — the article may benefit from recompilation.  &#x60;None&#x60; when not computed (e.g. in list responses). | [optional] 
**status** | **str** |  | 
**title** | **str** |  | [optional] 
**usage_score** | **float** |  | 
**version** | **int** |  | 

## Example

```python
from covalence.models.article_response import ArticleResponse

# TODO update the JSON string below
json = "{}"
# create an instance of ArticleResponse from a JSON string
article_response_instance = ArticleResponse.from_json(json)
# print the JSON string representation of the object
print(ArticleResponse.to_json())

# convert the object into a dict
article_response_dict = article_response_instance.to_dict()
# create an instance of ArticleResponse from a dict
article_response_from_dict = ArticleResponse.from_dict(article_response_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


