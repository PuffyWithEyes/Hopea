# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
EAPI=8
DISTUTILS_EXT=1
DISTUTILS_USE_PEP517=setuptools
PYTHON_COMPAT=( python3_{13,14} )
inherit distutils-r1 pypi
DESCRIPTION="Cryptographic primitives in the Cryptodome namespace"
HOMEPAGE="https://www.pycryptodome.org/"
LICENSE="BSD-2 Unlicense"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
