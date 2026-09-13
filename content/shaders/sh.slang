struct ThreeBandSH {
    float4 coeff0;
    float4 coeff1;
    float  coeff2;

    static const float C0 = 0.2820947918f;
    static const float C1 = 0.4886025119f;
    static const float C2 = 1.0925484306f;
    static const float C3 = 0.3153915653f;
    static const float C4 = 0.5462742153f;

    __init()
    {
        coeff0 = float4(0.0, 0.0, 0.0, 0.0);
        coeff1 = float4(0.0, 0.0, 0.0, 0.0);
        coeff2 = 0.0;
    }

    static func sh_basis(float3 dir) -> ThreeBandSH {
        ThreeBandSH sh;
        sh.coeff0 = float4(C0, -C1 * dir.y, C1 * dir.z, -C1 * dir.x);
        sh.coeff1 = float4(C2 * dir.x * dir.y, -C2 * dir.y * dir.z, C3 * (3.0 * dir.z * dir.z - 1.0), -C2 * dir.x * dir.z);
        sh.coeff2 = C4 * (dir.x * dir.x - dir.y * dir.y);
        return sh;
    }

    [mutating]
    func multiply(float value) {
        this.coeff0 *= value;
        this.coeff1 *= value;
        this.coeff2 *= value;
    }

    [mutating]
    func project(float3 dir, float value) {
        let basis = sh_basis(dir);

        this.coeff0 += basis.coeff0 * value;
        this.coeff1 += basis.coeff1 * value;
        this.coeff2 += basis.coeff2 * value;
    }

    func evaluate(float3 dir) -> float {
        let basis = sh_basis(dir);
        let result = dot(basis.coeff0, this.coeff0) + dot(basis.coeff1, this.coeff1) + basis.coeff2 * this.coeff2;
        return result;
    }
}

func evaluate_diffuse_sh(float4* coefficients, float3 normal) -> float3 {
    let n = float4(normal, 1.0);
    let linear = float3(dot(coefficients[0], n), dot(coefficients[1], n), dot(coefficients[2], n));
    let products = n.xyzz * n.yzzx;
    let quadratic = float3(dot(coefficients[3], products), dot(coefficients[4], products), dot(coefficients[5], products));
    return max(linear + quadratic + coefficients[6].xyz * (n.x * n.x - n.y * n.y), 0.0);
}
