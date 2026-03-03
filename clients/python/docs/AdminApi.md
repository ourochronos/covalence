# covalence.AdminApi

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**admin_maintenance**](AdminApi.md#admin_maintenance) | **POST** /admin/maintenance | POST /admin/maintenance
[**admin_stats**](AdminApi.md#admin_stats) | **GET** /admin/stats | GET /admin/stats


# **admin_maintenance**
> MaintenanceResponse admin_maintenance(maintenance_request)

POST /admin/maintenance

### Example


```python
import covalence
from covalence.models.maintenance_request import MaintenanceRequest
from covalence.models.maintenance_response import MaintenanceResponse
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
    api_instance = covalence.AdminApi(api_client)
    maintenance_request = covalence.MaintenanceRequest() # MaintenanceRequest | 

    try:
        # POST /admin/maintenance
        api_response = api_instance.admin_maintenance(maintenance_request)
        print("The response of AdminApi->admin_maintenance:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling AdminApi->admin_maintenance: %s\n" % e)
```



### Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **maintenance_request** | [**MaintenanceRequest**](MaintenanceRequest.md)|  | 

### Return type

[**MaintenanceResponse**](MaintenanceResponse.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: application/json
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**200** | Maintenance completed |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

# **admin_stats**
> StatsResponse admin_stats()

GET /admin/stats

### Example


```python
import covalence
from covalence.models.stats_response import StatsResponse
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
    api_instance = covalence.AdminApi(api_client)

    try:
        # GET /admin/stats
        api_response = api_instance.admin_stats()
        print("The response of AdminApi->admin_stats:\n")
        pprint(api_response)
    except Exception as e:
        print("Exception when calling AdminApi->admin_stats: %s\n" % e)
```



### Parameters

This endpoint does not need any parameter.

### Return type

[**StatsResponse**](StatsResponse.md)

### Authorization

No authorization required

### HTTP request headers

 - **Content-Type**: Not defined
 - **Accept**: application/json

### HTTP response details

| Status code | Description | Response headers |
|-------------|-------------|------------------|
**200** | System statistics |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

