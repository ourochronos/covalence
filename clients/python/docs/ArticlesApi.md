# covalence.ArticlesApi

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**compile_article**](ArticlesApi.md#compile_article) | **POST** /articles/compile | POST /articles/compile
[**get_article**](ArticlesApi.md#get_article) | **GET** /articles/{id} | GET /articles/{id}
[**merge_articles**](ArticlesApi.md#merge_articles) | **POST** /articles/merge | POST /articles/merge


# **compile_article**
> CompileJobResponse compile_article(compile_request)

POST /articles/compile

### Example


```python
import covalence
from covalence.models.compile_job_response import CompileJobResponse
from covalence.models.compile_request import CompileRequest
from covalence.rest import ApiException
from pprint import pprint

# Defining the host is optional and defaults to http://localhost
# See configuration.py for a list of all supported configuration parameters.
configuration = covalence.Configuration(
    host = "http://localhost"
)


# Enter a context with an instance of the API client
with covalence.ApiClient(configuration) as api_client:
    # Create an instance of the API class
    api_instance = covalence.ArticlesApi(api_client)
    compile_request = covalence.CompileRequest() # CompileRequest | 

    try:
        # POST /articles/compile
        api_response = api_instance.compile_article(compile_request)
        print("The response of ArticlesApi->compile_article:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling ArticlesApi->compile_article: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **compile_request** | [**CompileRequest**](CompileRequest.md)|  | 

### Return type

[**CompileJobResponse**](CompileJobResponse.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: application/json
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**202** | Compilation job accepted |  -  |
**400** | Bad request |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

# **get_article**
> ArticleResponse get_article(id)

GET /articles/{id}

### Example


```python
import covalence
from covalence.models.article_response import ArticleResponse
from covalence.rest import ApiException
from pprint import pprint

# Defining the host is optional and defaults to http://localhost
# See configuration.py for a list of all supported configuration parameters.
configuration = covalence.Configuration(
    host = "http://localhost"
)


# Enter a context with an instance of the API client
with covalence.ApiClient(configuration) as api_client:
    # Create an instance of the API class
    api_instance = covalence.ArticlesApi(api_client)
    id = 'id_example' # str | Article UUID

    try:
        # GET /articles/{id}
        api_response = api_instance.get_article(id)
        print("The response of ArticlesApi->get_article:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling ArticlesApi->get_article: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **id** | **str**| Article UUID | 

### Return type

[**ArticleResponse**](ArticleResponse.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: Not defined
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**200** | Article found |  -  |
**404** | Article not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

# **merge_articles**
> ArticleResponse merge_articles(merge_request)

POST /articles/merge

### Example


```python
import covalence
from covalence.models.article_response import ArticleResponse
from covalence.models.merge_request import MergeRequest
from covalence.rest import ApiException
from pprint import pprint

# Defining the host is optional and defaults to http://localhost
# See configuration.py for a list of all supported configuration parameters.
configuration = covalence.Configuration(
    host = "http://localhost"
)


# Enter a context with an instance of the API client
with covalence.ApiClient(configuration) as api_client:
    # Create an instance of the API class
    api_instance = covalence.ArticlesApi(api_client)
    merge_request = covalence.MergeRequest() # MergeRequest | 

    try:
        # POST /articles/merge
        api_response = api_instance.merge_articles(merge_request)
        print("The response of ArticlesApi->merge_articles:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling ArticlesApi->merge_articles: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **merge_request** | [**MergeRequest**](MergeRequest.md)|  | 

### Return type

[**ArticleResponse**](ArticleResponse.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: application/json
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**201** | Articles merged |  -  |
**400** | Bad request |  -  |
**404** | One or both articles not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

