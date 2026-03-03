# covalence.MemoryApi

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**recall_memory**](MemoryApi.md#recall_memory) | **POST** /memory/search | POST /memory/search
[**store_memory**](MemoryApi.md#store_memory) | **POST** /memory | POST /memory


# **recall_memory**
> List[Memory] recall_memory(recall_request)

POST /memory/search

### Example


```python
import covalence
from covalence.models.memory import Memory
from covalence.models.recall_request import RecallRequest
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
    api_instance = covalence.MemoryApi(api_client)
    recall_request = covalence.RecallRequest() # RecallRequest | 

    try:
        # POST /memory/search
        api_response = api_instance.recall_memory(recall_request)
        print("The response of MemoryApi->recall_memory:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling MemoryApi->recall_memory: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **recall_request** | [**RecallRequest**](RecallRequest.md)|  | 

### Return type

[**List[Memory]**](Memory.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: application/json
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**200** | Memory recall results |  -  |
**400** | Bad request |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

# **store_memory**
> Memory store_memory(store_memory_request)

POST /memory

### Example


```python
import covalence
from covalence.models.memory import Memory
from covalence.models.store_memory_request import StoreMemoryRequest
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
    api_instance = covalence.MemoryApi(api_client)
    store_memory_request = covalence.StoreMemoryRequest() # StoreMemoryRequest | 

    try:
        # POST /memory
        api_response = api_instance.store_memory(store_memory_request)
        print("The response of MemoryApi->store_memory:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling MemoryApi->store_memory: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **store_memory_request** | [**StoreMemoryRequest**](StoreMemoryRequest.md)|  | 

### Return type

[**Memory**](Memory.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: application/json
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**201** | Memory stored |  -  |
**400** | Bad request |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

