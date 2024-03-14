#
# browser.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

options(browser = function(url){
    .ps.ui.showUrl(url)
})
