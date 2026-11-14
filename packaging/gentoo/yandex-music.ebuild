# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
EAPI=8
DISTUTILS_USE_PEP517=hatchling
PYTHON_COMPAT=( python3_{13,14} )
PYPI_PN=yandex_music
inherit distutils-r1 pypi
DESCRIPTION="Python Yandex Music client used by Hopea"
HOMEPAGE="https://github.com/MarshalX/yandex-music-api"
LICENSE="LGPL-3"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
RDEPEND="dev-python/requests[${PYTHON_USEDEP}] dev-python/pysocks[${PYTHON_USEDEP}] dev-python/typing-extensions[${PYTHON_USEDEP}]"
