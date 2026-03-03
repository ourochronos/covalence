# covalence.SearchApi

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**search**](SearchApi.md#search) | **POST** /search | POST /search


# **search**
> List[SearchResult] search(search_request)

POST /search

### Example


```python
import covalence
from covalence.models.search_request import SearchRequest
from covalence.models.search_result import SearchResult
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
    api_instance = covalence.SearchApi(api_client)
    search_request = covalence.SearchRequest() # SearchRequest | 

    try:
        # POST /search
        api_response = api_instance.search(search_request)
        print("The response of SearchApi->search:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling SearchApi->search: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **search_request** | [**SearchRequest**](SearchRequest.md)|  | 

### Return type

[**List[SearchResult]**](SearchResult.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: application/json
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**200** | Search results |  -  |
**400** | Bad request |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

