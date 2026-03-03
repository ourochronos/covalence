# WeightsInput


## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**graph** | **float** |  | [optional] 
**lexical** | **float** |  | [optional] 
**structural** | **float** | Weight for the structural similarity dimension (covalence#52). When omitted, defaults to &#x60;0.0&#x60; when explicit weights are provided, or the strategy preset when using a strategy. | [optional] 
**vector** | **float** |  | [optional] 

## Example

```python
from covalence.models.weights_input import WeightsInput

# TODO update the JSON string below
json = "{}"
# create an instance of WeightsInput from a JSON string
weights_input_instance = WeightsInput.from_json(json)
# print the JSON string representation of the object
print(WeightsInput.to_json())

# convert the object into a dict
weights_input_dict = weights_input_instance.to_dict()
# create an instance of WeightsInput from a dict
weights_input_from_dict = WeightsInput.from_dict(weights_input_dict)
```
[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


