# rpic offline OCR models

Put one of these PaddleOCR/MNN model groups in this directory.

Recommended fast group:

```text
PP-OCRv5_mobile_det_fp16.mnn
PP-OCRv5_mobile_rec_fp16.mnn
ppocr_keys_v5.txt
```

Compatible groups:

```text
PP-OCRv5_mobile_det.mnn
PP-OCRv5_mobile_rec.mnn
ppocr_keys_v5.txt
```

```text
ch_PP-OCRv4_det_infer.mnn
ch_PP-OCRv4_rec_infer.mnn
ppocr_keys_v4.txt
```

rpic searches this directory next to the executable first. For development,
it also searches `models\ocr-fast` under the current working directory.

You can override the model directory:

```powershell
$env:RPIC_OCR_MODEL_DIR = "D:\models\ocr-fast"
```
