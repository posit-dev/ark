# Data Explorer - Convert to Code Feature

Backend implementation of "convert to code" for Positron's data explorer in R, allowing users to generate R code (dplyr syntax) that replicates their UI-based data manipulations (filters, sorting).

## Key Files

### Core Implementation
- `convert_to_code.rs` - Core conversion logic with traits and handlers + comprehensive tests
- `r_data_explorer.rs` - Data explorer integration with convert-to-code support

### Tests
- `../../../tests/data_explorer.rs` - Integration tests for data explorer including extensive sorting tests

## Architecture

The R implementation follows similar patterns to the Python implementation:

- **Trait-based design** for extensibility with `FilterHandler`, `SortHandler`, and `CodeConverter` traits
- **PipeBuilder** for clean pipe chain generation (similar to Python's `MethodChainBuilder`)
- **Comprehensive filter/sort handlers** with type-aware value formatting
- **Non-syntactic column name handling** using backticks when needed

## Future Enhancements

1. Consider a "tidyverse" syntax where stringr functions are used for text search filters
2. Handle "base" and "data.table" syntaxes in addition to dplyr

## Reference Implementation

For architectural reference, see the Python implementation:
- `../positron/extensions/positron-python/python_files/posit/positron/convert.py` - Core conversion logic
- `../positron/extensions/positron-python/python_files/posit/positron/data_explorer.py` - Main data explorer integration
- `../positron/extensions/positron-python/python_files/posit/positron/tests/test_convert.py` - Execution validation tests
