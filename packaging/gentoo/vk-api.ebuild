# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
EAPI=8
DISTUTILS_USE_PEP517=setuptools
PYTHON_COMPAT=( python3_{13,14} )
PYPI_PN=vk_api
inherit distutils-r1 pypi
DESCRIPTION="Python VK API client used by Hopea"
HOMEPAGE="https://github.com/python273/vk_api"
LICENSE="Apache-2.0"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
RDEPEND="dev-python/requests[${PYTHON_USEDEP}] dev-python/beautifulsoup4[${PYTHON_USEDEP}]"
