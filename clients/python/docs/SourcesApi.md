# covalence.SourcesApi

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**delete_source**](SourcesApi.md#delete_source) | **DELETE** /sources/{id} | DELETE /sources/{id}
[**get_source**](SourcesApi.md#get_source) | **GET** /sources/{id} | GET /sources/{id}
[**ingest_source**](SourcesApi.md#ingest_source) | **POST** /sources | POST /sources


# **delete_source**
> delete_source(id)

DELETE /sources/{id}

### Example


```python
import covalence
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
    api_instance = covalence.SourcesApi(api_client)
    id = 'id_example' # str | Source UUID

    try:
        # DELETE /sources/{id}
        api_instance.delete_source(id)
    except Exception as e:
        print("Exception when calling SourcesApi->delete_source: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **id** | **str**| Source UUID | 

### Return type

void (empty response body)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: Not defined
 - **Accept**: Not defined

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**204** | Source deleted |  -  |
**404** | Source not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

# **get_source**
> SourceResponse get_source(id)

GET /sources/{id}

### Example


```python
import covalence
from covalence.models.source_response import SourceResponse
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
    api_instance = covalence.SourcesApi(api_client)
    id = 'id_example' # str | Source UUID

    try:
        # GET /sources/{id}
        api_response = api_instance.get_source(id)
        print("The response of SourcesApi->get_source:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling SourcesApi->get_source: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **id** | **str**| Source UUID | 

### Return type

[**SourceResponse**](SourceResponse.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: Not defined
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**200** | Source found |  -  |
**404** | Source not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

# **ingest_source**
> SourceResponse ingest_source(ingest_request)

POST /sources

### Example


```python
import covalence
from covalence.models.ingest_request import IngestRequest
from covalence.models.source_response import SourceResponse
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
    api_instance = covalence.SourcesApi(api_client)
    ingest_request = covalence.IngestRequest() # IngestRequest | 

    try:
        # POST /sources
        api_response = api_instance.ingest_source(ingest_request)
        print("The response of SourcesApi->ingest_source:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling SourcesApi->ingest_source: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **ingest_request** | [**IngestRequest**](IngestRequest.md)|  | 

### Return type

[**SourceResponse**](SourceResponse.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: application/json
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**201** | Source ingested successfully |  -  |
**400** | Bad request |  -  |
**500** | Internal server error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

